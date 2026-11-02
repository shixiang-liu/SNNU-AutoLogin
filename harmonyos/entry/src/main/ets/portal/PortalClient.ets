import http from '@ohos.net.http';
import util from '@ohos.util';
import { BusinessError } from '@kit.BasicServicesKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

/**
 * 门户协议客户端（HarmonyOS NEXT / ArkTS）。
 *
 * 协议来源：src/portal.rs —— 本文件里的主机、端口表、表单字段、判定字符串与
 * 解析策略逐字对齐 Rust 实现，改动任何一项都必须两边同时改。
 *
 * 移动端原则（用户钦定，勿改）：手机端不做 Wi-Fi 自愈——连哪个 Wi-Fi 由系统
 * 自动化接管；App 只负责登录 / 状态 / 断开 + 应用存活期间的掉线重连。
 */

/** portal.rs:7  pub const HOST */
export const PORTAL_HOST: string = '202.117.144.205';

/** portal.rs:8-10  PORTS —— 顺序即优先级，首选 8601 */
export const PORTAL_PORTS: number[] = [8601, 8602, 8603, 8605, 8609, 8604, 8606, 8607, 8608, 8610, 8600];

/** portal.rs:12  const MARK = "当前登录账号" —— 登录成功判定字符串 */
export const PORTAL_MARK: string = '当前登录账号';

/** portal.rs:11  UA —— 与桌面端同一标识 */
const USER_AGENT: string =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36';

const ACCEPT_LANGUAGE: string = 'zh-CN,zh;q=0.9';

/** hilog 域与标签，与 EntryAbility 同域，便于 hdc hilog 一次过滤 */
const LOG_DOMAIN: number = 0x0000;
const LOG_TAG: string = 'PortalClient';

/**
 * 把一次异常压成一行可诊断文本。@ohos.net.http 失败时抛 BusinessError：
 * code 直接指出原因（2300006 域名解析失败 / 2300007 连不上服务器 / 2300028 超时），
 * 而 message 常常只有一句笼统的英文，所以两者都要带上，缺一个就难判断是 DNS、路由还是明文拦截。
 */
function describeError(err: Error): string {
  const error: BusinessError = err as BusinessError;
  const message: string = error.message;
  const code: number = error.code;
  // 用 typeof 而不是与 undefined 比较：BusinessError.code 声明为非空 number，
  // 但网络栈个别失败只带 message，运行时仍可能是 undefined，直接比会被类型检查判为无交集。
  const hasCode: boolean = typeof code === 'number' && code > 0;
  const hasMessage: boolean = typeof message === 'string' && message.length > 0;
  if (hasCode && hasMessage) {
    return `code=${code} ${message}`;
  }
  if (hasMessage) {
    return message;
  }
  if (hasCode) {
    return `code=${code}`;
  }
  return '未知错误';
}

/** 统一失败日志。占位符用 %{public}s，避免错误文本里的 % 被当成格式符 */
function logFailure(where: string, port: number, err: Error): void {
  hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s port=%{public}d err=%{public}s',
    where, port, describeError(err));
}

/** 单端口探测超时；11 个端口并行探测，整体受这一预算约束 */
const PROBE_TIMEOUT_MS: number = 1500;
/** 缓存端口的快速确认超时（portal.rs:240 用 500ms） */
const CACHED_PROBE_TIMEOUT_MS: number = 800;
/** 状态页读取超时 */
const STATUS_TIMEOUT_MS: number = 4000;
/** 登录提交超时（portal.rs:769 用 10s） */
const LOGIN_TIMEOUT_MS: number = 10000;
/** 断开超时（portal.rs:908 用 6s） */
const LOGOFF_TIMEOUT_MS: number = 6000;

export enum PortalKind {
  ONLINE = 'online',
  OFFLINE = 'offline',
  UNREACHABLE = 'unreachable'
}

/** 状态页解析出的在线会话 */
export class PortalSession {
  account: string;
  ip: string;
  loginTime: string;

  constructor(account: string, ip: string, loginTime: string) {
    this.account = account;
    this.ip = ip;
    this.loginTime = loginTime;
  }

  /** 学号部分（account 形如 20241234@mobile） */
  user(): string {
    const at: number = this.account.indexOf('@');
    return at < 0 ? this.account : this.account.substring(0, at);
  }

  /** 运营商部分（mobile / unicom / 空） */
  operator(): string {
    const at: number = this.account.indexOf('@');
    return at < 0 ? '' : this.account.substring(at + 1);
  }
}

export class PortalStatus {
  kind: PortalKind;
  session: PortalSession | null;
  port: number;

  constructor(kind: PortalKind, session: PortalSession | null, port: number) {
    this.kind = kind;
    this.session = session;
    this.port = port;
  }

  static unreachable(): PortalStatus {
    return new PortalStatus(PortalKind.UNREACHABLE, null, 0);
  }
}

class RawResponse {
  code: number;
  location: string;
  cookie: string;
  bytes: ArrayBuffer | null;

  constructor(code: number, location: string, cookie: string, bytes: ArrayBuffer | null) {
    this.code = code;
    this.location = location;
    this.cookie = cookie;
    this.bytes = bytes;
  }
}

export function sleep(ms: number): Promise<void> {
  return new Promise<void>((resolve: () => void) => {
    setTimeout(() => resolve(), ms);
  });
}

export class PortalClient {
  /** 缓存的可用端口，0 表示尚未探测成功（对齐 portal.rs 的 state.port） */
  private preferredPort: number = 0;
  /**
   * 最近一次端口探测失败的原因，形如「port=8601 code=2300006 Couldn't resolve host name」。
   * locate() 只回一个 unreachable 状态，界面需要一句技术细节才判断得出是 DNS、路由还是明文拦截，
   * 所以由探测层记下第一手原因，login() 的报错文案再把它带出去。
   */
  private probeError: string = '';

  /** 最近一次探测失败原因；空串表示没有记录 */
  lastProbeError(): string {
    return this.probeError;
  }

  cachedPort(): number {
    return this.preferredPort;
  }

  private static base(port: number): string {
    return `http://${PORTAL_HOST}:${port}/snnuportal`;
  }

  // ---------------------------------------------------------------- HTTP 基础

  private static async request(
    url: string, method: http.RequestMethod, extraHeader: Record<string, string>,
    body: string, connectMs: number, readMs: number): Promise<RawResponse> {
    const client: http.HttpRequest = http.createHttp();
    const headers: Record<string, string> = {
      'User-Agent': USER_AGENT,
      'Accept-Language': ACCEPT_LANGUAGE,
      'Cache-Control': 'no-cache'
    };
    const keys: string[] = Object.keys(extraHeader);
    for (let i = 0; i < keys.length; i++) {
      headers[keys[i]] = extraHeader[keys[i]];
    }
    const options: http.HttpRequestOptions = {
      method: method,
      header: headers,
      extraData: body,
      expectDataType: http.HttpDataType.ARRAY_BUFFER,
      connectTimeout: connectMs,
      readTimeout: readMs,
      usingCache: false
    };
    try {
      const response: http.HttpResponse = await client.request(url, options);
      let bytes: ArrayBuffer | null = null;
      if (response.result instanceof ArrayBuffer) {
        bytes = response.result as ArrayBuffer;
      }
      let location: string = '';
      let cookie: string = '';
      const responseHeader: Record<string, string> = response.header as Record<string, string>;
      const loc: string = responseHeader['location'];
      if (loc !== undefined && loc !== null) {
        location = loc;
      }
      const rawCookie: string = responseHeader['set-cookie'];
      if (rawCookie !== undefined && rawCookie !== null && rawCookie.length > 0) {
        cookie = PortalClient.joinCookies(rawCookie);
      }
      return new RawResponse(response.responseCode as number, location, cookie, bytes);
    } catch (err) {
      // 单个端口的真实失败原因；调用方只看到 null，因此必须在这里留痕
      const port: string = PortalClient.portOf(url);
      hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s port=%{public}s url=%{public}s err=%{public}s',
        'request 失败', port, url, describeError(err as Error));
      throw err as Error;
    } finally {
      client.destroy();
    }
  }

  /** 从门户 URL 里取出端口，只为日志标注；取不到时返回 '?' 而不是抛错 */
  private static portOf(url: string): string {
    const start: number = url.indexOf('://');
    if (start < 0) {
      return '?';
    }
    const colon: number = url.indexOf(':', start + 3);
    if (colon < 0) {
      return '?';
    }
    const slash: number = url.indexOf('/', colon);
    return slash < 0 ? url.substring(colon + 1) : url.substring(colon + 1, slash);
  }

  /** 一次响应可能有多个 Set-Cookie（JSESSIONID 之外还有负载均衡 Cookie），全部带回 */
  private static joinCookies(raw: string): string {
    const lines: string[] = raw.split('\n');
    const pairs: string[] = [];
    for (let i = 0; i < lines.length; i++) {
      const first: string = lines[i].split(';')[0].trim();
      if (first.length > 0 && first.indexOf('=') > 0) {
        pairs.push(first);
      }
    }
    return pairs.join('; ');
  }

  // ---------------------------------------------------------------- 解码

  /**
   * portal.rs:111-119 decode —— 先按 UTF-8 严格解码，出现替换字符再回退 GB18030。
   * 顺序不能反：先按 GB18030 解会把 UTF-8 中文变成乱码。
   */
  static decode(bytes: ArrayBuffer | null): string {
    if (bytes === null || bytes.byteLength === 0) {
      return '';
    }
    const view: Uint8Array = new Uint8Array(bytes);
    let text: string = '';
    try {
      const utf8: util.TextDecoder = util.TextDecoder.create('utf-8');
      text = utf8.decodeToString(view);
    } catch (err) {
      text = '';
    }
    if (text.length > 0 && text.indexOf('\uFFFD') < 0) {
      return text.startsWith('\uFEFF') ? text.substring(1) : text;
    }
    // 状态页实测是 gb2312，用 GB18030 超集解码；个别版本不支持该名时退回 gbk。
    const encodings: string[] = ['gb18030', 'gbk'];
    for (let i = 0; i < encodings.length; i++) {
      try {
        const decoder: util.TextDecoder = util.TextDecoder.create(encodings[i]);
        const decoded: string = decoder.decodeToString(view);
        return decoded.startsWith('\uFEFF') ? decoded.substring(1) : decoded;
      } catch (err) {
        continue;
      }
    }
    return text;
  }

  // ---------------------------------------------------------------- 表单

  /**
   * portal.rs:773-779 的 5 个字段，顺序与字面值完全一致：
   * sourceurl=null&account=&password=&yys=&issave=
   * yys 只取 mobile / unicom / 空（telecom 已停用）。
   */
  static formBody(account: string, password: string, operator: string): string {
    return 'sourceurl=null' +
      '&account=' + encodeURIComponent(account) +
      '&password=' + encodeURIComponent(password) +
      '&yys=' + encodeURIComponent(operator) +
      '&issave=';
  }

  // ---------------------------------------------------------------- 解析

  /**
   * 取隐藏输入框的值：id="account" value="42512012@mobile"。
   *
   * 只用 ASCII 的 id + 值做锚点，不匹配中文标签——状态页是 gb2312，
   * 中文标签解码后可能乱码，而 id / value 恒为 ASCII。
   * 与 ios/shortcuts-guide.md 已验证的正则 id="account" value="([^"]*)" 同一策略，
   * 这里放宽为属性顺序任意、单双引号均可。
   */
  static hiddenValue(html: string, id: string): string {
    const pattern: RegExp = new RegExp('id=["\']' + id + '["\'][^>]*?value=["\']([^"\']*)["\']', 'i');
    const matched = html.match(pattern);
    if (matched !== null && matched.length > 1) {
      return matched[1];
    }
    const reversed: RegExp =
      new RegExp('value=["\']([^"\']*)["\'][^>]*?id=["\']' + id + '["\']', 'i');
    const back = html.match(reversed);
    if (back !== null && back.length > 1) {
      return back[1];
    }
    return '';
  }

  /** 登录时间形如 2026-10-25 14:06:29，纯数字，不受乱码影响 */
  static loginTimeOf(html: string): string {
    const matched = html.match(/20\d{2}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/);
    return matched === null ? '' : matched[0];
  }

  /** 页面是否仍是登录表单（离线态）。portal.rs:365-383 的字段判定 */
  static hasLoginForm(html: string): boolean {
    const low: string = html.toLowerCase();
    const accountField: boolean = low.indexOf('name="account"') >= 0 ||
      low.indexOf("name='account'") >= 0 ||
      low.indexOf('id="account"') >= 0 ||
      low.indexOf("id='account'") >= 0;
    const passwordField: boolean = low.indexOf('name="password"') >= 0 ||
      low.indexOf("name='password'") >= 0 ||
      low.indexOf('id="password"') >= 0 ||
      low.indexOf("id='password'") >= 0;
    return accountField && passwordField;
  }

  /** 状态页 HTML → 在线会话；取不到账号即视为不在线（portal.rs:222-229） */
  static sessionOf(html: string): PortalSession | null {
    let account: string = PortalClient.hiddenValue(html, 'account');
    if (account.length === 0) {
      // 隐藏框缺失时退回中文标签解析（gb18030 解码成功时可用）
      account = PortalClient.accountAfterMark(html);
    }
    if (account.length === 0) {
      return null;
    }
    let ip: string = PortalClient.hiddenValue(html, 'ipaddr');
    if (ip.length === 0) {
      ip = PortalClient.valueAfterLabel(html, '当前IP');
    }
    return new PortalSession(account, ip, PortalClient.loginTimeOf(html));
  }

  /** portal.rs:140-169 take_account —— 取「当前登录账号」之后的第一段合法值 */
  private static accountAfterMark(html: string): string {
    const idx: number = html.indexOf(PORTAL_MARK);
    if (idx < 0) {
      return '';
    }
    const rest: string = html.substring(idx + PORTAL_MARK.length).replace(/<[^>]*>/g, ' ');
    const parts: string[] = rest.split(/\s+/);
    for (let i = 0; i < parts.length; i++) {
      const token: string = parts[i].replace(/^[：:]+/, '');
      if (/^[A-Za-z0-9@._-]{4,128}$/.test(token)) {
        return token;
      }
    }
    return '';
  }

  /** portal.rs:173-209 take_value —— 取标签后的第一段值 */
  private static valueAfterLabel(html: string, label: string): string {
    const idx: number = html.indexOf(label);
    if (idx < 0) {
      return '';
    }
    const rest: string = html.substring(idx + label.length)
      .replace(/<br\s*\/?>/gi, '\n')
      .replace(/<\/(td|tr|p|div)>/gi, '\n');
    const lines: string[] = rest.split(/[\r\n]/);
    for (let i = 0; i < lines.length; i++) {
      const value: string = lines[i]
        .replace(/<[^>]*>/g, ' ')
        .replace(/&nbsp;/g, ' ')
        .replace(/^[\s：:]+/, '')
        .trim();
      if (value.length > 0) {
        return value.substring(0, Math.min(value.length, 64));
      }
    }
    return '';
  }

  // ---------------------------------------------------------------- 状态

  /** 单端口状态探测；连不上返回 null（portal.rs:359-384 status_at） */
  async statusAt(port: number, timeoutMs: number): Promise<PortalStatus | null> {
    let body: string = '';
    try {
      const response: RawResponse = await PortalClient.request(
        `${PortalClient.base(port)}/userstatus.jsp`, http.RequestMethod.GET, {}, '',
        timeoutMs, timeoutMs);
      if (response.code >= 300 && response.code < 400) {
        // 状态页被跳到登录页时跟随一次；跨地址跳转一律不跟随（portal.rs:399-415）。
        const target: string = this.resolveRedirect(port, response.location);
        if (target.length === 0) {
          // 跳转目标跨主机/跨端口：不是这台门户的地址，本端口判定不可用
          hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s port=%{public}d code=%{public}d location=%{public}s',
            'statusAt 跳转目标不可跟随', port, response.code, response.location);
          return null;
        }
        const next: RawResponse = await PortalClient.request(
          target, http.RequestMethod.GET, {}, '', timeoutMs, timeoutMs);
        if (next.code !== 200) {
          hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s port=%{public}d code=%{public}d',
            'statusAt 跟随跳转后非 200', port, next.code);
          return null;
        }
        body = PortalClient.decode(next.bytes);
      } else if (response.code === 200) {
        body = PortalClient.decode(response.bytes);
      } else {
        hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s port=%{public}d code=%{public}d',
          'statusAt 响应码异常', port, response.code);
        if (this.probeError.length === 0) {
          this.probeError = `port=${port} HTTP ${response.code}`;
        }
        return null;
      }
    } catch (err) {
      // 探测失败的主因都落在这里：DNS 解析不了、路由不通、明文 HTTP 被系统拦截
      const reason: string = `port=${port} ${describeError(err as Error)}`;
      logFailure('statusAt 请求失败', port, err as Error);
      // 只留第一条。11 个端口是并行探测的，先抛出的那个不一定是端口表里的第一个，
      // 但同一台机器上各端口失败原因通常相同（同一个 DNS / 同一条路由），取一条足以判断性质。
      if (this.probeError.length === 0) {
        this.probeError = reason;
      }
      return null;
    }
    const session: PortalSession | null = PortalClient.sessionOf(body);
    if (session !== null) {
      return new PortalStatus(PortalKind.ONLINE, session, port);
    }
    if (PortalClient.hasLoginForm(body)) {
      return new PortalStatus(PortalKind.OFFLINE, null, port);
    }
    return null;
  }

  /** 跳转目标必须是同主机同端口的 /snnuportal/ 路径 */
  private resolveRedirect(port: number, location: string): string {
    if (location.length === 0) {
      return '';
    }
    if (location.startsWith('http://')) {
      const prefix: string = `http://${PORTAL_HOST}:${port}`;
      return location.startsWith(prefix + '/snnuportal/') ? location : '';
    }
    if (location.startsWith('/') && !location.startsWith('//')) {
      return `http://${PORTAL_HOST}:${port}${location}`;
    }
    if (location.indexOf('://') < 0 && !location.startsWith('//')) {
      return `${PortalClient.base(port)}/${location}`;
    }
    return '';
  }

  /**
   * 11 端口 failover：先试缓存端口，再并行探测其余端口（portal.rs:232-261 同语义）——
   * 在线结果优先于离线结果；同为离线时按端口表顺序取排名靠前的那个。
   * 探测成功的端口写回缓存。
   */
  async locate(): Promise<PortalStatus> {
    this.probeError = '';
    if (this.preferredPort > 0) {
      const cached: PortalStatus | null =
        await this.statusAt(this.preferredPort, CACHED_PROBE_TIMEOUT_MS);
      if (cached !== null) {
        return cached;
      }
    }
    const ordered: number[] = [];
    for (let i = 0; i < PORTAL_PORTS.length; i++) {
      if (PORTAL_PORTS[i] !== this.preferredPort) {
        ordered.push(PORTAL_PORTS[i]);
      }
    }
    const probes: Promise<PortalStatus | null>[] = [];
    for (let i = 0; i < ordered.length; i++) {
      probes.push(this.statusAt(ordered[i], PROBE_TIMEOUT_MS));
    }
    const results: (PortalStatus | null)[] = await Promise.all(probes);
    let offline: PortalStatus | null = null;
    let online: PortalStatus | null = null;
    for (let i = 0; i < results.length; i++) {
      const status: PortalStatus | null = results[i];
      if (status === null) {
        continue;
      }
      if (status.kind === PortalKind.ONLINE) {
        if (online === null) {
          online = status;
        }
      } else if (status.kind === PortalKind.OFFLINE && offline === null) {
        offline = status;
      }
    }
    const chosen: PortalStatus | null = online !== null ? online : offline;
    if (chosen === null) {
      return PortalStatus.unreachable();
    }
    this.preferredPort = chosen.port;
    return chosen;
  }

  // ---------------------------------------------------------------- 登录

  /**
   * portal.rs:754-833 login_unlocked 的移动端等价实现：
   * 1. 先 GET login.jsp 拿会话 Cookie（验证码要求会随刷新消失）；
   * 2. POST /snnuportal/login，5 字段表单；
   * 3. 响应含「当前登录账号」即成功；
   * 4. 否则按门户原文给出错误提示。
   */
  async login(account: string, password: string, operator: string): Promise<PortalStatus> {
    const current: PortalStatus = await this.locate();
    if (current.kind === PortalKind.UNREACHABLE) {
      const detail: string = this.probeError;
      throw new Error(detail.length > 0
        ? `连不上认证服务器（${detail}）`
        : '连不上认证服务器（全部端口无响应）');
    }
    if (current.kind === PortalKind.ONLINE) {
      return current;
    }
    const port: number = current.port;
    const loginUrl: string = `${PortalClient.base(port)}/login`;
    let cookie: string = await this.openLoginPage(port);
    let lastError: string = '';
    for (let attempt = 0; attempt < 2; attempt++) {
      const header: Record<string, string> = {
        'Content-Type': 'application/x-www-form-urlencoded',
        'Referer': `${PortalClient.base(port)}/login.jsp`
      };
      if (cookie.length > 0) {
        header['Cookie'] = cookie;
      }
      let response: RawResponse;
      try {
        response = await PortalClient.request(loginUrl, http.RequestMethod.POST, header,
          PortalClient.formBody(account, password, operator), LOGIN_TIMEOUT_MS, LOGIN_TIMEOUT_MS);
      } catch (err) {
        const detail: string = describeError(err as Error);
        hilog.error(LOG_DOMAIN, LOG_TAG, '%{public}s port=%{public}d err=%{public}s',
          'login 提交失败', port, detail);
        throw new Error(`连不上认证服务器（port=${port}: ${detail}）`);
      }
      const html: string = PortalClient.decode(response.bytes);
      if (html.indexOf(PORTAL_MARK) >= 0) {
        const session: PortalSession | null = PortalClient.sessionOf(html);
        return new PortalStatus(PortalKind.ONLINE,
          session !== null ? session : new PortalSession(account, '', ''), port);
      }
      const error: string = PortalClient.classifyError(html);
      lastError = error;
      if (PortalClient.needsCaptcha(error)) {
        // 刷新登录页可让验证码要求消失，与 portal.rs:796-810 的重试策略一致
        if (attempt + 1 < 2) {
          cookie = await this.openLoginPage(port);
          await sleep(200);
          continue;
        }
        throw new Error(error);
      }
      // 响应不是验证码：用在线状态兜底确认是否其实已登录（portal.rs:812-819）
      const after: PortalStatus | null = await this.statusAt(port, STATUS_TIMEOUT_MS);
      if (after !== null && after.kind === PortalKind.ONLINE && after.session !== null &&
        after.session.user() === account) {
        return after;
      }
      throw new Error(error);
    }
    throw new Error(lastError.length > 0 ? lastError : '认证没有通过');
  }

  /** portal.rs:866-891 open_login_page —— 顺带把会话 Cookie 带回来 */
  private async openLoginPage(port: number): Promise<string> {
    try {
      const response: RawResponse = await PortalClient.request(
        `${PortalClient.base(port)}/login.jsp?_=${Date.now()}`, http.RequestMethod.GET,
        { 'Accept': 'text/html,application/xhtml+xml' }, '', 5000, 5000);
      return response.cookie;
    } catch (err) {
      // 拿不到 Cookie 不致命（后续请求仍会发出），但失败原因要留痕，否则登录失败时无从判断
      logFailure('openLoginPage 失败', port, err as Error);
      return '';
    }
  }

  // ---------------------------------------------------------------- 断开

  /** portal.rs:903-921 logout —— GET /snnuportal/logoff，没有 .jsp 后缀 */
  async logout(): Promise<boolean> {
    const current: PortalStatus = await this.locate();
    if (current.kind === PortalKind.OFFLINE) {
      return true;
    }
    if (current.kind !== PortalKind.ONLINE) {
      return false;
    }
    const port: number = current.port;
    try {
      await PortalClient.request(`${PortalClient.base(port)}/logoff`, http.RequestMethod.GET,
        {}, '', LOGOFF_TIMEOUT_MS, LOGOFF_TIMEOUT_MS);
    } catch (err) {
      return false;
    }
    for (let i = 0; i < 3; i++) {
      const after: PortalStatus | null = await this.statusAt(port, 3000);
      if (after !== null && after.kind === PortalKind.OFFLINE) {
        return true;
      }
      await sleep(300);
    }
    return false;
  }

  // ---------------------------------------------------------------- 错误文案

  /**
   * portal.rs:538-632 classify / 673-692 friendly_login_error 的收敛版：
   * 凭据错误优先于验证码（门户常在密码错误页上顺带显示验证码），
   * 凭据错误统一成一句可操作提示，其余保留门户原文。
   */
  static classifyError(html: string): string {
    const text: string = PortalClient.plainText(html);
    if (text.length === 0) {
      return '账号或密码错误，请检查后重试';
    }
    const credential: string[] = ['密码错误', '密码输入错误', '密码不正确', '密码有误', '密码无效',
      '密码不匹配', '密码验证失败', '账号密码有误', '账号或密码', '用户名或密码', '账号不存在',
      '用户不存在', '未开通', '已停机', '停机', '欠费', '余额不足', '绑定失败'];
    for (let i = 0; i < credential.length; i++) {
      if (text.indexOf(credential[i]) >= 0) {
        return '您的账号密码有误';
      }
    }
    const captcha: string[] = ['验证码错误', '验证码不正确', '验证码无效', '验证码已过期',
      '请输入验证码', '验证码不能为空'];
    for (let i = 0; i < captcha.length; i++) {
      if (text.indexOf(captcha[i]) >= 0) {
        return captcha[i];
      }
    }
    if (text.toLowerCase().indexOf('captcha') >= 0) {
      return '请输入验证码';
    }
    const words: string[] = ['失败', '错误', '无效', '已过期', '认证', '在线数'];
    for (let i = 0; i < words.length; i++) {
      if (text.indexOf(words[i]) >= 0) {
        return PortalClient.limit(text);
      }
    }
    if (PortalClient.hasLoginForm(html)) {
      return '账号或密码错误，请检查后重试';
    }
    return '认证没有通过';
  }

  static needsCaptcha(message: string): boolean {
    return message.indexOf('验证码') >= 0;
  }

  /** portal.rs:456-496 clean_text 的等价实现：去脚本/样式/注释/标签，规整空白 */
  static plainText(html: string): string {
    let text: string = html
      .replace(/<script[^>]*>[\s\S]*?<\/script>/gi, ' ')
      .replace(/<style[^>]*>[\s\S]*?<\/style>/gi, ' ')
      .replace(/<noscript[^>]*>[\s\S]*?<\/noscript>/gi, ' ')
      .replace(/<!--[\s\S]*?-->/g, ' ')
      .replace(/<[^>]*>/g, ' ')
      .replace(/&nbsp;/g, ' ')
      .replace(/&lt;/g, '<')
      .replace(/&gt;/g, '>')
      .replace(/&quot;/g, '"')
      .replace(/&amp;/g, '&')
      .replace(/[\s\u3000]+/g, ' ')
      .trim();
    if (text.startsWith('陕西师范大学网络Portal')) {
      text = text.substring('陕西师范大学网络Portal'.length).trim();
    }
    return text;
  }

  private static limit(text: string): string {
    return text.length > 120 ? text.substring(0, 120) : text;
  }

  /** 日志脱敏：门户错误页可能回显学号，落盘/上通知前先掩掉（portal.rs:995-1018） */
  static mask(account: string): string {
    const at: number = account.indexOf('@');
    const user: string = at < 0 ? account : account.substring(0, at);
    if (user.length <= 4) {
      return '***';
    }
    return `${user.substring(0, 3)}***${user.substring(user.length - 2)}`;
  }
}
