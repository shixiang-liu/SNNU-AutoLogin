import { common } from '@kit.AbilityKit';
import { preferences } from '@kit.ArkData';
import { asset } from '@kit.AssetStoreKit';
import { util } from '@kit.ArkTS';
import { BusinessError } from '@kit.BasicServicesKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

/**
 * 凭据持久化：学号 / 上网方式走 Preferences，密码走 Asset Store Kit。
 *
 * 密码不能与学号同放 Preferences：Preferences 是明文键值文件，任何拿到应用沙箱的
 * 途径都能读走。Asset 由系统加密保管（等价 iOS 钥匙串 / Windows DPAPI），
 * 并且 ACL 默认只有本应用可访问，所以两者必须分开存。
 */

const LOG_DOMAIN: number = 0x0000;
const LOG_TAG: string = 'CredentialStore';

/** Preferences 文件名与键名；改键名等于让老用户的已存学号失效，勿随意动 */
const PREF_NAME: string = 'snnu_login';
const KEY_ACCOUNT: string = 'account';
const KEY_OPERATOR: string = 'operator';
const KEY_REMEMBER: string = 'remember';

/** Asset 记录别名。同一别名重复 add 会报 DUPLICATED，因此写入前先删后加 */
const ASSET_ALIAS: string = 'snnu_portal_password';

/** 一次读回的完整凭据；remember 为 false 时密码为空串 */
export class SavedCredential {
  account: string;
  password: string;
  operator: string;
  remember: boolean;

  constructor(account: string, password: string, operator: string, remember: boolean) {
    this.account = account;
    this.password = password;
    this.operator = operator;
    this.remember = remember;
  }

  static empty(): SavedCredential {
    return new SavedCredential('', '', '', true);
  }
}

/** Asset 的别名与密钥都用字节串，两个方向各写一次，别处不要再手搓 */
function bytesOf(text: string): Uint8Array {
  return util.TextEncoder.create('utf-8').encodeInto(text);
}

function textOf(bytes: Uint8Array): string {
  return util.TextDecoder.create('utf-8').decodeToString(bytes);
}

/**
 * 密码读写。所有失败都不向上抛：凭据存不下属于降级场景，
 * 不该让「记住密码」的故障拦住用户登录，但必须在 hilog 里留下原因。
 */
export class CredentialStore {
  private context: common.Context | null = null;
  private prefs: preferences.Preferences | null = null;

  bind(context: common.Context): void {
    this.context = context;
  }

  /** 惰性打开 Preferences：同名方法会遮蔽导入的 preferences 命名空间，故用 handle 命名 */
  private async handle(): Promise<preferences.Preferences | null> {
    if (this.prefs !== null) {
      return this.prefs;
    }
    if (this.context === null) {
      return null;
    }
    try {
      const options: preferences.Options = { name: PREF_NAME };
      this.prefs = await preferences.getPreferences(this.context, options);
      return this.prefs;
    } catch (err) {
      hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s err=%{public}s',
        '打开 Preferences 失败', (err as BusinessError).message);
      return null;
    }
  }

  /** 启动时读回填输入框。任何一步缺失都退化成「没有存过」，不报错 */
  async load(): Promise<SavedCredential> {
    const store: preferences.Preferences | null = await this.handle();
    if (store === null) {
      return SavedCredential.empty();
    }
    try {
      const remember: boolean = store.getSync(KEY_REMEMBER, true) as boolean;
      const account: string = store.getSync(KEY_ACCOUNT, '') as string;
      const operator: string = store.getSync(KEY_OPERATOR, '') as string;
      // 用户关掉「记住密码」后，Asset 里的旧密码必须一并作废，
      // 否则开关只挡住了界面回填，密码仍留在系统里。
      const password: string = remember ? await this.readPassword() : '';
      return new SavedCredential(account, password, operator, remember);
    } catch (err) {
      hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s err=%{public}s',
        '读取 Preferences 失败', (err as BusinessError).message);
      return SavedCredential.empty();
    }
  }

  /** 登录成功后写入。remember=false 时只存学号与运营商，并清掉已存的密码 */
  async save(account: string, password: string, operator: string, remember: boolean): Promise<void> {
    const store: preferences.Preferences | null = await this.handle();
    if (store === null) {
      return;
    }
    try {
      // 空账号只代表「调用方手上没有账号」（例如刚关掉开关），不能拿它覆盖已存的学号
      if (account.length > 0) {
        store.putSync(KEY_ACCOUNT, account);
        store.putSync(KEY_OPERATOR, operator);
      }
      store.putSync(KEY_REMEMBER, remember);
      await store.flush();
    } catch (err) {
      hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s err=%{public}s',
        '写入 Preferences 失败', (err as BusinessError).message);
    }
    if (remember) {
      await this.writePassword(password);
    } else {
      await this.clearPassword();
    }
  }

  /** 用户主动清除：开关关闭或退出登录时调用，两步都做，避免残留 */
  async clear(): Promise<void> {
    const store: preferences.Preferences | null = await this.handle();
    if (store !== null) {
      try {
        store.deleteSync(KEY_ACCOUNT);
        store.deleteSync(KEY_OPERATOR);
        await store.flush();
      } catch (err) {
        hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s err=%{public}s',
          '清除 Preferences 失败', (err as BusinessError).message);
      }
    }
    await this.clearPassword();
  }

  private async readPassword(): Promise<string> {
    const query: asset.AssetMap = new Map();
    query.set(asset.Tag.ALIAS, bytesOf(ASSET_ALIAS));
    query.set(asset.Tag.RETURN_TYPE, asset.ReturnType.ALL);
    try {
      const found: Array<asset.AssetMap> = await asset.query(query);
      if (found.length === 0) {
        return '';
      }
      const secret: asset.Value | undefined = found[0].get(asset.Tag.SECRET);
      if (secret === undefined) {
        return '';
      }
      return textOf(secret as Uint8Array);
    } catch (err) {
      const error: BusinessError = err as BusinessError;
      // 没存过密码是正常路径（首次启动 / 用户关过开关），不当成错误刷日志
      if (error.code !== asset.ErrorCode.NOT_FOUND) {
        hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s code=%{public}d err=%{public}s',
          '读取密码失败', error.code, error.message);
      }
      return '';
    }
  }

  private async writePassword(password: string): Promise<void> {
    // 先删后加：别名唯一，重复 add 会抛 DUPLICATED；remove 找不到记录时抛 NOT_FOUND，忽略即可
    await this.clearPassword();
    const attributes: asset.AssetMap = new Map();
    attributes.set(asset.Tag.ALIAS, bytesOf(ASSET_ALIAS));
    attributes.set(asset.Tag.SECRET, bytesOf(password));
    // 开机即可读（DEVICE_POWERED_ON）：本应用只在用户已解锁的界面里读写，
    // 用 DEVICE_UNLOCKED 会在锁屏期间让后台轮询拿不到凭据。
    attributes.set(asset.Tag.ACCESSIBILITY, asset.Accessibility.DEVICE_POWERED_ON);
    attributes.set(asset.Tag.DATA_LABEL_NORMAL_1, bytesOf('snnu'));
    try {
      await asset.add(attributes);
    } catch (err) {
      const error: BusinessError = err as BusinessError;
      hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s code=%{public}d err=%{public}s',
        '写入密码失败', error.code, error.message);
    }
  }

  private async clearPassword(): Promise<void> {
    const query: asset.AssetMap = new Map();
    query.set(asset.Tag.ALIAS, bytesOf(ASSET_ALIAS));
    try {
      await asset.remove(query);
    } catch (err) {
      const error: BusinessError = err as BusinessError;
      if (error.code !== asset.ErrorCode.NOT_FOUND) {
        hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s code=%{public}d err=%{public}s',
          '清除密码失败', error.code, error.message);
      }
    }
  }
}
