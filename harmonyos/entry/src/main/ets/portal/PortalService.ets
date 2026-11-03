import { common, wantAgent, WantAgent } from '@kit.AbilityKit';
import { backgroundTaskManager } from '@kit.BackgroundTasksKit';
import { notificationManager } from '@kit.NotificationKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { PortalClient, PortalKind, PortalStatus, PortalSession } from './PortalClient';
import { CredentialStore, SavedCredential } from './CredentialStore';

/**
 * 前台轮询服务：对齐上游 Android 1.0.0 的 LoginService.java
 * （前台服务 + 常驻通知 + 定时轮询 + 掉线重登录）。
 *
 * 移动端原则（用户钦定，勿改）：本服务**不做 Wi-Fi 自愈**——连接哪个 Wi-Fi
 * 由系统自动化接管；它只负责在 App 存活期间保持认证不掉线。
 *
 * HarmonyOS 的对应形态是「长时任务」：先申请 ohos.permission.KEEP_BACKGROUND_RUNNING
 * 权限，再 backgroundTaskManager.startBackgroundRunning 挂一个 continuous task，
 * 之后在应用进程存活期间按固定间隔轮询门户状态。
 */

/** 轮询间隔 15s。上游 Android 前台态 5s / 后台态 30s，这里取中间值兼顾耗电与及时性。 */
export const POLL_INTERVAL_MS: number = 15000;

/** 掉线重连失败后的退避表，与 portal.rs:1151 next_delay 的 Other 分支一致 */
const RETRY_DELAYS_SEC: number[] = [5, 10, 20, 60];

const NOTICE_ID: number = 1001;

/** 服务对外暴露的一次快照，UI 与通知共用 */
export class ServiceSnapshot {
  kind: PortalKind;
  title: string;
  detail: string;
  account: string;
  ip: string;
  loginTime: string;
  running: boolean;

  constructor(kind: PortalKind, title: string, detail: string, running: boolean,
    session: PortalSession | null) {
    this.kind = kind;
    this.title = title;
    this.detail = detail;
    this.running = running;
    this.account = session === null ? '' : session.account;
    this.ip = session === null ? '' : session.ip;
    this.loginTime = session === null ? '' : session.loginTime;
  }

  static idle(): ServiceSnapshot {
    return new ServiceSnapshot(PortalKind.UNREACHABLE, '未开始', '点击「状态」检查当前认证', false, null);
  }
}

export type SnapshotListener = (snapshot: ServiceSnapshot) => void;

export class PortalService {
  private static instance: PortalService | null = null;

  private client: PortalClient = new PortalClient();
  private context: common.UIAbilityContext | null = null;
  private timer: number = -1;
  private running: boolean = false;
  private polling: boolean = false;
  private attempt: number = 0;
  private snapshot: ServiceSnapshot = ServiceSnapshot.idle();
  private listener: SnapshotListener | null = null;

  /**
   * 凭据落盘：学号 / 上网方式在 Preferences，密码在 Asset Store Kit 加密存储。
   * 这里的字段是轮询用的内存副本，来源与去向都经过 CredentialStore。
   */
  private store: CredentialStore = new CredentialStore();
  private account: string = '';
  private password: string = '';
  private operator: string = '';
  private remember: boolean = true;

  static shared(): PortalService {
    if (PortalService.instance === null) {
      PortalService.instance = new PortalService();
    }
    return PortalService.instance;
  }

  bind(context: common.UIAbilityContext): void {
    this.context = context;
    this.store.bind(context);
  }

  /** 启动时读回已保存的凭据，同时刷进轮询用的内存副本 */
  async restore(): Promise<SavedCredential> {
    const saved: SavedCredential = await this.store.load();
    this.account = saved.account;
    this.operator = saved.operator;
    this.password = saved.password;
    this.remember = saved.remember;
    return saved;
  }

  onSnapshot(listener: SnapshotListener): void {
    this.listener = listener;
    listener(this.snapshot);
  }

  current(): ServiceSnapshot {
    return this.snapshot;
  }

  isRunning(): boolean {
    return this.running;
  }

  hasCredentials(): boolean {
    return this.account.length > 0 && this.password.length > 0;
  }

  private publish(snapshot: ServiceSnapshot): void {
    this.snapshot = snapshot;
    if (this.listener !== null) {
      this.listener(snapshot);
    }
    this.refreshNotification();
  }

  private state(kind: PortalKind, title: string, detail: string): ServiceSnapshot {
    return new ServiceSnapshot(kind, title, detail, this.running, null);
  }

  // ---------------------------------------------------------------- 手动操作

  /** 「记住密码」开关：关掉时立刻删掉已存的密码，不等下一次登录 */
  async setRemember(remember: boolean): Promise<void> {
    this.remember = remember;
    if (remember) {
      return;
    }
    // 账号为空时 save 会跳过学号，只清密码，不会把已存的学号抹成空串
    await this.store.save(this.account, '', this.operator, false);
  }

  /** 用户点了「登录」即保存凭据：网络不通（如模拟器）时也已留存，下次打开自动回填 */
  async saveCredentials(account: string, password: string, operator: string, remember: boolean): Promise<void> {
    this.account = account;
    this.password = password;
    this.operator = operator;
    this.remember = remember;
    await this.store.save(account, password, operator, remember);
  }

  /** 手动登录：成功即接管轮询（用户显式点了登录，视为开启自动重连） */
  async login(account: string, password: string, operator: string): Promise<ServiceSnapshot> {
    this.account = account;
    this.password = password;
    this.operator = operator;
    this.publish(this.state(PortalKind.UNREACHABLE, '正在登录', '正在提交认证…'));
    try {
      const status: PortalStatus = await this.client.login(account, password, operator);
      this.attempt = 0;
      // 认证通过才落盘：存下错的密码会让下次启动直接失败，还看不出是自己输错了
      await this.store.save(account, password, operator, this.remember);
      const snapshot: ServiceSnapshot = new ServiceSnapshot(PortalKind.ONLINE, '已登录',
        PortalService.describe(status.session), this.running, status.session);
      this.publish(snapshot);
      return snapshot;
    } catch (err) {
      const message: string = (err as Error).message;
      this.publish(this.state(PortalKind.OFFLINE, '登录失败', message));
      throw new Error(message);
    }
  }

  /** 手动查状态（不触发重登录） */
  async refresh(): Promise<ServiceSnapshot> {
    this.publish(this.state(this.snapshot.kind, '正在查询', '正在读取门户状态…'));
    const status: PortalStatus = await this.client.locate();
    const snapshot: ServiceSnapshot = this.snapshotOf(status);
    this.publish(snapshot);
    return snapshot;
  }

  /** 手动断开：成功后停止轮询，避免刚断开又被自动登回去 */
  async logout(): Promise<ServiceSnapshot> {
    this.publish(this.state(this.snapshot.kind, '正在断开', '正在通知门户下线…'));
    const ok: boolean = await this.client.logout();
    if (ok) {
      await this.stopPolling();
      const snapshot: ServiceSnapshot = this.state(PortalKind.OFFLINE, '已断开', '当前未登录校园网');
      this.publish(snapshot);
      return snapshot;
    }
    const snapshot: ServiceSnapshot =
      this.state(this.snapshot.kind, '断开未确认', '门户没有返回离线状态，请稍后重试');
    this.publish(snapshot);
    return snapshot;
  }

  // ---------------------------------------------------------------- 长时任务

  /** 开启掉线重连：申请长时任务 → 常驻通知 → 15s 轮询 */
  async startPolling(): Promise<void> {
    if (this.running) {
      return;
    }
    await this.startContinuousTask();
    this.running = true;
    this.attempt = 0;
    this.refreshNotification();
    this.scheduleNext(0);
  }

  async stopPolling(): Promise<void> {
    this.running = false;
    if (this.timer >= 0) {
      clearTimeout(this.timer);
      this.timer = -1;
    }
    await this.stopContinuousTask();
    this.cancelNotification();
  }

  private async startContinuousTask(): Promise<void> {
    if (this.context === null) {
      return;
    }
    try {
      const info: wantAgent.WantAgentInfo = {
        wants: [
          {
            bundleName: this.context.abilityInfo.bundleName,
            abilityName: 'EntryAbility'
          }
        ],
        actionType: wantAgent.OperationType.START_ABILITY,
        requestCode: 0,
        actionFlags: [wantAgent.WantAgentFlags.UPDATE_PRESENT_FLAG]
      };
      const agent: WantAgent = await wantAgent.getWantAgent(info);
      await backgroundTaskManager.startBackgroundRunning(this.context,
        backgroundTaskManager.BackgroundMode.DATA_TRANSFER, agent);
    } catch (err) {
      // 长时任务被系统拒绝（未在 AGC 配置 / 用户限制）时降级为前台轮询，不阻断登录。
      const error: BusinessError = err as BusinessError;
      console.warn(`startBackgroundRunning 失败: ${error.code} ${error.message}`);
    }
  }

  private async stopContinuousTask(): Promise<void> {
    if (this.context === null) {
      return;
    }
    try {
      await backgroundTaskManager.stopBackgroundRunning(this.context);
    } catch (err) {
      const error: BusinessError = err as BusinessError;
      console.warn(`stopBackgroundRunning 失败: ${error.code} ${error.message}`);
    }
  }

  // ---------------------------------------------------------------- 轮询

  private scheduleNext(delayMs: number): void {
    if (!this.running) {
      return;
    }
    if (this.timer >= 0) {
      clearTimeout(this.timer);
    }
    this.timer = setTimeout(() => {
      this.tick();
    }, delayMs);
  }

  private async tick(): Promise<void> {
    if (!this.running || this.polling) {
      return;
    }
    this.polling = true;
    let nextDelay: number = POLL_INTERVAL_MS;
    try {
      const status: PortalStatus = await this.client.locate();
      if (status.kind === PortalKind.ONLINE) {
        this.attempt = 0;
        this.publish(this.snapshotOf(status));
      } else if (status.kind === PortalKind.OFFLINE) {
        nextDelay = await this.relogin();
      } else {
        // 轮询失败同样要给出技术细节：这是用户最常看到的那条状态，只说「确认已连接」无从判断原因
        const detail: string = this.client.lastProbeError();
        this.publish(this.state(PortalKind.UNREACHABLE, '连不上门户',
          detail.length > 0
            ? `${detail}，确认已连接 SNNU 校园网后会自动重试`
            : '确认已连接 SNNU 校园网后会自动重试'));
      }
    } catch (err) {
      this.publish(this.state(PortalKind.UNREACHABLE, '轮询异常', (err as Error).message));
    }
    this.polling = false;
    this.scheduleNext(nextDelay);
  }

  /** 掉线重登录：返回下一轮轮询的间隔（毫秒）。只在用户已配置凭据时进行 */
  private async relogin(): Promise<number> {
    if (!this.hasCredentials()) {
      this.publish(this.state(PortalKind.OFFLINE, '当前未登录', '请先填写学号与密码'));
      return POLL_INTERVAL_MS;
    }
    this.publish(this.state(PortalKind.OFFLINE, '掉线重连中', '检测到未登录，正在自动提交认证…'));
    try {
      const status: PortalStatus = await this.client.login(this.account, this.password, this.operator);
      this.attempt = 0;
      this.publish(this.snapshotOf(status));
      return POLL_INTERVAL_MS;
    } catch (err) {
      const message: string = (err as Error).message;
      // 凭据错误 / 验证码 / 账号状态：重试没有意义，停在这里等用户处理
      // （portal.rs:1146-1149 next_delay 对这几类返回 None，即熔断）
      if (PortalService.isFatal(message)) {
        await this.stopPolling();
        this.publish(this.state(PortalKind.OFFLINE, '自动重连已停止', message));
        return POLL_INTERVAL_MS;
      }
      // 其余错误按 portal.rs:1151 的 5/10/20/60 秒退避重试
      const delaySec: number = RETRY_DELAYS_SEC[Math.min(this.attempt, RETRY_DELAYS_SEC.length - 1)];
      this.attempt = Math.min(this.attempt + 1, RETRY_DELAYS_SEC.length - 1);
      this.publish(this.state(PortalKind.UNREACHABLE, '重连失败',
        `${message}（${delaySec} 秒后重试）`));
      return delaySec * 1000;
    }
  }

  /** portal.rs:1096-1143 fail_kind 的移动端子集 */
  private static isFatal(message: string): boolean {
    const fatalWords: string[] = ['验证码', '密码', '账号', '欠费', '余额', '停机', '停用', '锁定',
      '未开通', '已过期', '禁用', '在线数'];
    for (let i = 0; i < fatalWords.length; i++) {
      if (message.indexOf(fatalWords[i]) >= 0) {
        return true;
      }
    }
    return false;
  }

  private snapshotOf(status: PortalStatus): ServiceSnapshot {
    if (status.kind === PortalKind.ONLINE) {
      return new ServiceSnapshot(PortalKind.ONLINE, '已登录',
        PortalService.describe(status.session), this.running, status.session);
    }
    if (status.kind === PortalKind.OFFLINE) {
      return this.state(PortalKind.OFFLINE, '当前未登录', `门户端口 ${status.port} 可达，尚未认证`);
    }
    // 带上探测层记下的第一手失败原因：界面直接显示 DNS / 路由 / 明文拦截的区别，
    // 用户不必再去翻 hilog。没有记录时（例如从没探测过）退回原来的笼统提示。
    const detail: string = this.client.lastProbeError();
    return this.state(PortalKind.UNREACHABLE, '连不上门户',
      detail.length > 0
        ? `11 个候选端口都不可达（${detail}），请确认已连接 SNNU 校园网`
        : '11 个候选端口都不可达，请确认已连接 SNNU 校园网');
  }

  private static describe(session: PortalSession | null): string {
    if (session === null) {
      return '已登录';
    }
    const parts: string[] = [];
    parts.push(`账号 ${PortalClient.mask(session.account)}`);
    if (session.ip.length > 0) {
      parts.push(`IP ${session.ip}`);
    }
    if (session.loginTime.length > 0) {
      parts.push(`登录于 ${session.loginTime}`);
    }
    return parts.join(' · ');
  }

  // ---------------------------------------------------------------- 通知

  /** 首次运行申请通知授权：长时任务的常驻通知与状态提示都要它 */
  async requestNotification(): Promise<void> {
    if (this.context === null) {
      return;
    }
    try {
      const enabled: boolean = await notificationManager.isNotificationEnabled();
      if (!enabled) {
        await notificationManager.requestEnableNotification(this.context);
      }
    } catch (err) {
      const error: BusinessError = err as BusinessError;
      console.warn(`申请通知授权失败: ${error.code} ${error.message}`);
    }
  }

  private refreshNotification(): void {
    if (this.context === null || !this.running) {
      return;
    }
    const request: notificationManager.NotificationRequest = {
      id: NOTICE_ID,
      content: {
        notificationContentType: notificationManager.ContentType.NOTIFICATION_CONTENT_BASIC_TEXT,
        normal: {
          title: this.snapshot.title,
          text: this.snapshot.detail
        }
      }
    };
    notificationManager.publish(request).catch((err: BusinessError) => {
      console.warn(`通知发布失败: ${err.code} ${err.message}`);
    });
  }

  private cancelNotification(): void {
    if (this.context === null) {
      return;
    }
    notificationManager.cancel(NOTICE_ID).catch((err: BusinessError) => {
      console.warn(`通知取消失败: ${err.code} ${err.message}`);
    });
  }
}
