import { PortalService, ServiceSnapshot } from '../portal/PortalService';
import { PortalKind } from '../portal/PortalClient';
import { SavedCredential } from '../portal/CredentialStore';

/**
 * 主界面：学号 / 密码 / 上网方式 + 登录、状态、断开三个动作 + 状态展示区。
 *
 * 移动端原则（用户钦定，勿改）：手机端不做 Wi-Fi 自愈，连 Wi-Fi 由系统自动化接管；
 * 本页只承载登录 / 状态 / 断开与应用存活期间的掉线重连开关。
 */

/** 上网方式：yys 只允许 mobile / unicom / 空（telecom 已停用） */
class OperatorOption {
  label: string;
  value: string;

  constructor(label: string, value: string) {
    this.label = label;
    this.value = value;
  }
}

const OPERATORS: OperatorOption[] = [
  new OperatorOption('校园网', ''),
  new OperatorOption('移动', 'mobile'),
  new OperatorOption('联通', 'unicom')
];

/** 已存的 operator 值 → 按钮下标。存的值不认识时退回第一项（校园网），不抛错 */
function operatorIndexOf(value: string): number {
  for (let i = 0; i < OPERATORS.length; i++) {
    if (OPERATORS[i].value === value) {
      return i;
    }
  }
  return 0;
}

@Entry
@Component
struct Index {
  @State account: string = '';
  @State password: string = '';
  @State operatorIndex: number = 0;
  @State autoReconnect: boolean = false;
  @State remember: boolean = true;
  @State busy: boolean = false;
  @State title: string = '未开始';
  @State detail: string = '点击「状态」检查当前认证';
  @State online: boolean = false;

  private service: PortalService = PortalService.shared();

  aboutToAppear(): void {
    this.service.requestNotification();
    this.service.onSnapshot((snapshot: ServiceSnapshot) => {
      this.title = snapshot.title;
      this.detail = snapshot.detail;
      this.online = snapshot.kind === PortalKind.ONLINE;
      this.autoReconnect = snapshot.running;
    });
    this.restoreCredentials();
  }

  /** 启动时把上次存的学号 / 运营商 / 密码填回输入框，省去每次重输 */
  private async restoreCredentials(): Promise<void> {
    try {
      const saved: SavedCredential = await this.service.restore();
      this.account = saved.account;
      this.password = saved.password;
      this.operatorIndex = operatorIndexOf(saved.operator);
      this.remember = saved.remember;
    } catch (err) {
      // 读回失败不影响使用，只是要重输一次；原因已在 CredentialStore 里记进 hilog
      this.detail = '读取已保存的账号失败，请重新输入';
    }
  }

  private currentOperator(): string {
    return OPERATORS[this.operatorIndex].value;
  }

  private toast(message: string): void {
    // 用状态区展示，不额外弹窗，避免每次轮询都打扰用户
    this.detail = message;
  }

  private validate(): boolean {
    if (this.account.trim().length === 0) {
      this.toast('请填写学号');
      return false;
    }
    if (this.password.length === 0) {
      this.toast('请填写密码');
      return false;
    }
    return true;
  }

  private async doLogin(): Promise<void> {
    if (this.busy || !this.validate()) {
      return;
    }
    // 点登录即保存：网络不通时（如模拟器）凭据也已留存，下次打开自动回填；
    // 密码是否保存跟随「记住密码」开关，用户主动输入即代表授权
    await this.service.saveCredentials(this.account.trim(), this.password, this.currentOperator(), this.remember);
    this.busy = true;
    try {
      await this.service.login(this.account.trim(), this.password, this.currentOperator());
      if (!this.service.isRunning()) {
        await this.service.startPolling();
      }
    } catch (err) {
      this.toast((err as Error).message);
    }
    this.busy = false;
  }

  private async doStatus(): Promise<void> {
    if (this.busy) {
      return;
    }
    this.busy = true;
    try {
      await this.service.refresh();
    } catch (err) {
      this.toast((err as Error).message);
    }
    this.busy = false;
  }

  private async doLogout(): Promise<void> {
    if (this.busy) {
      return;
    }
    this.busy = true;
    try {
      await this.service.logout();
      this.autoReconnect = false;
    } catch (err) {
      this.toast((err as Error).message);
    }
    this.busy = false;
  }

  private async toggleRemember(on: boolean): Promise<void> {
    this.remember = on;
    try {
      await this.service.setRemember(on);
    } catch (err) {
      this.toast((err as Error).message);
    }
  }

  private async toggleAuto(on: boolean): Promise<void> {
    this.autoReconnect = on;
    if (on) {
      if (!this.validate()) {
        this.autoReconnect = false;
        return;
      }
      await this.service.startPolling();
    } else {
      await this.service.stopPolling();
    }
  }

  build() {
    Column({ space: 16 }) {
      Text($r('app.string.app_title'))
        .fontSize(26)
        .fontWeight(FontWeight.Bold)
        .width('100%')
        .textAlign(TextAlign.Center)

      TextInput({ placeholder: $r('app.string.label_account'), text: this.account })
        .type(InputType.Number)
        .maxLength(20)
        .width('100%')
        .onChange((value: string) => {
          this.account = value;
        })

      TextInput({ placeholder: $r('app.string.label_password'), text: this.password })
        .type(InputType.Password)
        .maxLength(64)
        .width('100%')
        .onChange((value: string) => {
          this.password = value;
        })

      Row({ space: 8 }) {
        Text($r('app.string.label_operator')).fontSize(14).fontColor('#666666')
        ForEach(OPERATORS, (item: OperatorOption, index: number) => {
          Button(item.label)
            .fontSize(14)
            .height(32)
            .backgroundColor(this.operatorIndex === index ? '#1A6FDB' : '#E8E8E8')
            .fontColor(this.operatorIndex === index ? Color.White : '#333333')
            .onClick(() => {
              this.operatorIndex = index;
            })
        }, (item: OperatorOption) => item.label)
      }
      .width('100%')
      .justifyContent(FlexAlign.Start)

      Row({ space: 12 }) {
        Button($r('app.string.action_login'))
          .layoutWeight(1)
          .enabled(!this.busy)
          .onClick(() => {
            this.doLogin();
          })
        Button($r('app.string.action_status'))
          .layoutWeight(1)
          .backgroundColor('#5B8FF9')
          .enabled(!this.busy)
          .onClick(() => {
            this.doStatus();
          })
        Button($r('app.string.action_logout'))
          .layoutWeight(1)
          .backgroundColor('#F4664A')
          .enabled(!this.busy)
          .onClick(() => {
            this.doLogout();
          })
      }
      .width('100%')

      Row({ space: 8 }) {
        Text($r('app.string.switch_auto')).fontSize(14).layoutWeight(1)
        Toggle({ type: ToggleType.Switch, isOn: this.autoReconnect })
          .onChange((isOn: boolean) => {
            this.toggleAuto(isOn);
          })
      }
      .width('100%')

      Row({ space: 8 }) {
        Text($r('app.string.switch_remember')).fontSize(14).layoutWeight(1)
        Toggle({ type: ToggleType.Switch, isOn: this.remember })
          .onChange((isOn: boolean) => {
            this.toggleRemember(isOn);
          })
      }
      .width('100%')

      Column({ space: 8 }) {
        Text($r('app.string.status_title')).fontSize(12).fontColor('#999999').width('100%')
        Text(this.title)
          .fontSize(18)
          .fontWeight(FontWeight.Medium)
          .fontColor(this.online ? '#00A870' : '#333333')
          .width('100%')
        Text(this.detail)
          .fontSize(13)
          .fontColor('#666666')
          .width('100%')
          .textAlign(TextAlign.Start)
      }
      .width('100%')
      .padding(16)
      .borderRadius(12)
      .backgroundColor('#F5F6F8')
      .alignItems(HorizontalAlign.Start)

      Blank()

      Text('手机端不做 Wi-Fi 自愈：连接哪个 Wi-Fi 由系统自动化接管，本应用只负责认证与掉线重连。')
        .fontSize(11)
        .fontColor('#999999')
        .width('100%')
        .textAlign(TextAlign.Center)
    }
    .width('100%')
    .height('100%')
    .padding(20)
  }
}
