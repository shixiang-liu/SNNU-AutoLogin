# 协议对齐说明：PortalClient.ets ↔ src/portal.rs

本文件是 H2 验收的对照表。左列为桌面端 Rust 实现的位置与字面值，右列为 HarmonyOS 端 ArkTS 实现。
**任何一项改动都必须两边同时改。**

## 1. 主机与端口表

| 项 | src/portal.rs | PortalClient.ets |
| :--- | :--- | :--- |
| 主机 | `portal.rs:7` `pub const HOST: &str = "202.117.144.205"` | `PORTAL_HOST = '202.117.144.205'` |
| 端口表（11 个，顺序即优先级） | `portal.rs:8-10` `PORTS: [u16; 11] = [8601, 8602, 8603, 8605, 8609, 8604, 8606, 8607, 8608, 8610, 8600]` | `PORTAL_PORTS = [8601, 8602, 8603, 8605, 8609, 8604, 8606, 8607, 8608, 8610, 8600]` |
| User-Agent | `portal.rs:11` `Mozilla/5.0 (Windows NT 10.0; Win64; x64) ... Chrome/131.0.0.0 Safari/537.36` | `USER_AGENT`（同一字符串） |
| 成功判定字符串 | `portal.rs:12` `const MARK = "当前登录账号"` | `PORTAL_MARK = '当前登录账号'` |

## 2. 表单（5 字段，顺序与字面值）

`portal.rs:773-779`：

```rust
let form = vec![
    ("sourceurl", "null"),
    ("account", account),
    ("password", password),
    ("yys", operator),
    ("issave", ""),
];
```

`PortalClient.ets` 的 `formBody()`：

```ts
return 'sourceurl=null' +
  '&account=' + encodeURIComponent(account) +
  '&password=' + encodeURIComponent(password) +
  '&yys=' + encodeURIComponent(operator) +
  '&issave=';
```

| 字段 | 取值 | 说明 |
| :--- | :--- | :--- |
| `sourceurl` | `null` | 固定字符串 `null`，不是空值 |
| `account` | 学号 | URL 编码 |
| `password` | 密码 | URL 编码 |
| `yys` | `mobile` / `unicom` / 空 | telecom 已停用，界面只给三项：校园网 / 移动 / 联通 |
| `issave` | 空字符串 | 固定留空，**必须保留 `issave=` 这个空值字段** |

运行时断言（见 `H3` 的自查脚本输出）：

```
formBody('20241234', 'p@ss&word', 'mobile')
  === 'sourceurl=null&account=20241234&password=p%40ss%26word&yys=mobile&issave='
formBody('1', '2', '') === 'sourceurl=null&account=1&password=2&yys=&issave='
```

## 3. 端点与 HTTP 方法

| 动作 | portal.rs | PortalClient.ets |
| :--- | :--- | :--- |
| 状态页 | `portal.rs:390` `GET {base}/snnuportal/userstatus.jsp` | `GET ${base}/userstatus.jsp` |
| 登录页（取 Cookie） | `portal.rs:868` `GET {base}/snnuportal/login.jsp?_=<ms>` | `GET ${base}/login.jsp?_=${Date.now()}` |
| 登录提交 | `portal.rs:765` `POST {base}/snnuportal/login` | `POST ${base}/login` |
| 登录 Referer | `portal.rs:768` `{base}/snnuportal/login.jsp` | `${base}/login.jsp` |
| 断开 | `portal.rs:908` `GET {base}/snnuportal/logoff`（**无 .jsp 后缀**） | `GET ${base}/logoff` |
| 超时 | 登录 10s、状态 4s、断开 6s | `LOGIN_TIMEOUT_MS=10000` / `STATUS_TIMEOUT_MS=4000` / `LOGOFF_TIMEOUT_MS=6000` |

`base = http://202.117.144.205:<port>/snnuportal`

## 4. 11 端口 failover

`portal.rs:232-261 status()` 的语义：

1. 若已缓存端口，先用 **500ms 预算**快速确认该端口；成功直接返回。
2. 否则把缓存端口排到最前，其余按 `PORTS` 顺序，**并行探测**，共享一个截止时间。
3. 在线结果优先于离线结果；同为离线时按排名（端口表顺序）取靠前的。
4. 成功的端口写回缓存。

`PortalClient.ets` 的 `locate()`：

1. `preferredPort > 0` 时先 `statusAt(preferredPort, CACHED_PROBE_TIMEOUT_MS=800)`。
2. 其余端口按 `PORTAL_PORTS` 顺序 `Promise.all` 并行探测，每端口 `PROBE_TIMEOUT_MS=1500`。
3. `online` 优先于 `offline`，同级取端口表顺序靠前者。
4. `this.preferredPort = chosen.port`。

> 差异：桌面端用线程 + mpsc 共享一个 deadline，ArkTS 侧用并行 Promise 各自计时。
> 端口集合、顺序、优先级、缓存语义完全一致。

## 5. 状态页解析（ASCII 正则，抗 gb2312 乱码）

门户状态页实测是 gb2312，中文标签解码后可能乱码，但下面这组隐藏输入框是**纯 ASCII**：

```html
<input type="hidden" id="ipaddr"  value="10.138.108.230"/>
<input type="hidden" id="account" value="42512012@mobile"/>
```

| 目标 | 方案 | PortalClient.ets |
| :--- | :--- | :--- |
| 账号 | `ios/shortcuts-guide.md` 已验证的正则 `id="account" value="([^"]*)"` | `hiddenValue(html, 'account')`，正则放宽为属性顺序任意、单双引号均可 |
| 当前 IP | 同上，`id="ipaddr"` | `hiddenValue(html, 'ipaddr')` |
| 登录时间 | `20\d{2}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}`（纯数字，不受乱码影响） | `loginTimeOf(html)` |

**离线判定**（`portal.rs:365-383`）：页面同时含 `account` 与 `password` 字段
（`name="..."` / `name='...'` / `id="..."` / `id='...'` 四种写法之一）即为登录表单，判 `Offline`。
→ `hasLoginForm(html)`。

**在线判定**：`sessionOf(html)` 取到账号即 `Online`（`portal.rs:222-229`）。

## 6. 解码策略

`portal.rs:111-119`：**先 UTF-8 严格解码**，失败才回退 GB18030，避免把 UTF-8 中文当成 GBK 变乱码。

`PortalClient.decode()`：`util.TextDecoder.create('utf-8').decodeToString()` → 若结果含 `U+FFFD` 替换字符，
改用 `util.TextDecoder.create('gb18030')`；个别设备不支持 `gb18030` 名时再退 `gbk`。
两者都去掉 BOM。

## 7. 成功 / 失败判定

| 场景 | 判定 | 实现 |
| :--- | :--- | :--- |
| 登录成功 | 响应正文含 `当前登录账号`（`portal.rs:791`） | `html.indexOf(PORTAL_MARK) >= 0` |
| 兜底确认 | 响应不含 MARK 时，再查一次在线状态；账号一致也算成功（`portal.rs:812-819`） | `statusAt()` + `session.user() === account` |
| 验证码 | 错误文案含「验证码」→ 刷新登录页重试一次（`portal.rs:796-810`） | `needsCaptcha()` + 第二次 `openLoginPage()` |
| 凭据错误 | 命中凭据词表 → 统一收敛为「您的账号密码有误」（`portal.rs:673-692`） | `classifyError()` |
| 熔断 | 凭据 / 验证码 / 账号状态类错误**不再自动重试**（`portal.rs:1146-1149`） | `PortalService.isFatal()` → `stopPolling()` |
| 退避重试 | 其余错误按 5/10/20/60 秒重试（`portal.rs:1150-1155`） | `RETRY_DELAYS_SEC = [5, 10, 20, 60]`，由 `relogin()` 返回下一轮间隔 |

## 8. 已声明的差异（有意为之）

| 项 | 桌面端 | HarmonyOS 端 | 原因 |
| :--- | :--- | :--- | :--- |
| 端口发现 | 额外探测 `connect.rom.miui.com/generate_204` 等公开地址的跳转 | 未实现 | 移动端不做 Wi-Fi 自愈，也不需要发现新门户 IP |
| 主机缓存 | `state.json` 持久化 `portal_host` | 只缓存端口，主机固定 | 校园门户地址不会漂移，持久化反而增加攻击面 |
| 代理环境 | `try_proxy_from_env(false)` | 不设代理 | 系统级设置 |
| 表单编码 | `send_form`（ureq 内部百分号编码） | `encodeURIComponent` | 编码规则一致：`&`→`%26`、`@`→`%40`、`+`→`%2B` |
