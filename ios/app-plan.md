# iOS 方案三：原生 App（ipa）技术方案

> 状态：**方案与骨架代码**，未编译、未签名、未上机。本机为 Windows，无 Swift / Xcode 工具链，
> 代码仅做静态自查，见第 5 节。
> 前置阅读：[ipa-roadmap.md](ipa-roadmap.md)（成本与分发路径）、[shortcuts-guide.md](shortcuts-guide.md)（零成本等效方案）

---

## 0. 结论速览

| 问题 | 结论 |
| :--- | :--- |
| 技术路线 | **纯 Swift / SwiftUI**，不引入 Rust FFI |
| 能 1:1 复刻安卓吗 | **登录 / 状态 / 断开：能**；**掉线秒级自愈：不能**（iOS 无前台服务） |
| 后台能跑多久 | `BGAppRefreshTask` 由系统调度，**一天几次到十几次不等，且不保证**；单次约 30 秒 |
| 唯一接近安卓前台服务的机制 | `NEPacketTunnelProvider`（VPN 扩展），需付费开发者账号 + Network Extension 权限 |
| 最低成本路径 | AltStore / SideStore 自签（0 元，7 天重签） |
| 推荐度 | 想「点图标即用 + 自动补登录」→ **快捷指令已够**；想「原生界面 + 后台兜底」→ 值得做，但别指望秒级自愈 |

---

## 1. 技术路线评估

### 1.1 候选路线

| 维度 | 路线 A：纯 Swift / SwiftUI | 路线 B：Rust 核心 + FFI（uniffi / cargo） |
| :--- | :--- | :--- |
| 复用现有代码 | 手写约 200 行 Swift 复刻 `portal.rs` 的 HTTP 逻辑 | 直接复用 `portal.rs` 与 `wifi_heal_core.rs` |
| 构建复杂度 | 一个 Xcode 工程，`swift build` 级 | 需 `cargo` + `uniffi` + 为 iOS 目标（`aarch64-apple-ios`）交叉编译 + 生成 Swift 绑定 + 打包 xcframework |
| 无 Mac 时可行性 | 云 Mac（Codemagic / GitHub Actions）可完整构建 | **同上，且要多配一套 Rust 交叉编译链**，CI 更重 |
| 二进制体积 | 小（几百 KB） | 大（Rust 运行时 + 未裁剪标准库，通常几 MB） |
| 调试成本 | 低，纯 Swift 栈 | 高，Swift ↔ Rust 边界崩溃难查 |
| 可复用的「核心价值」 | 登录/状态/断开三件事本身就是三次 HTTP | `wifi_heal_core` 的三条件决策**依赖 Wi-Fi 控制权**，iOS 没有 |
| 长期维护 | 单一语言 | 两套工具链 + 绑定代码 |

### 1.2 推荐：路线 A（纯 Swift）

理由三条：

1. **iOS 端的自愈本就不做**。`wifi_heal_core.rs` 的价值是「判定该不该动 Wi-Fi，然后真的去动」。
   iOS 既不能读 Wi-Fi 信号质量，也不能主动连接/切换网络，把决策核心搬过来**只剩一个算不出结果的空壳**。
2. **要复刻的逻辑量很小**。`portal.rs` 里真正需要在 iOS 重建的只有三件事：
   5 字段表单 POST、解析 `userstatus.jsp`、`GET /logoff`——合计约 200 行 Swift，见 `app/PortalClient.swift`。
3. **构建链越短，7 天重签越不容易崩**。自签路线本来就要反复重签，再叠一套 Rust 交叉编译链是给自己找事。

> 何时才考虑路线 B：如果将来 iOS 端也要做「和桌面端完全一致的多端口探测 + 状态机 + 熔断」这类复杂逻辑，
> 且希望三端共用一份经过测试的核心代码，那时 FFI 的收益才压过成本。当前需求下不值得。

---

## 2. 常驻能力：诚实边界

这是整个 iOS 方案里**唯一真正的硬伤**，必须讲清楚。

### 2.1 安卓有什么，iOS 没有什么

| 能力 | Android（上游 1.0.0） | iOS |
| :--- | :--- | :--- |
| 前台服务常驻 | ✅ 通知栏常驻，进程活着 | ❌ **无对应机制** |
| 后台定时轮询（秒级 / 分钟级） | ✅ `AlarmManager` / 前台服务循环 | ❌ 系统不提供 |
| 网络状态变化回调 | ✅ `ConnectivityManager` | 🟡 部分：`NWPathMonitor` 仅**前台**可靠 |
| 掉线即重连 | ✅ 秒级 | ❌ 做不到 |

### 2.2 iOS 的后台机制实测边界

| 机制 | 能否定时跑 | 真实限制 |
| :--- | :--- | :--- |
| `BGAppRefreshTask`（后台 App 刷新） | 🟡 勉强 | **由系统调度，不是定时器**。`earliestBeginDate` 只是「不早于」，实际间隔从几十分钟到一整天不等；系统会按用户使用习惯、电量、网络状况决定是否给你机会；单次执行预算约 **30 秒**；用户手动上滑杀掉 App 后**不再调度** |
| `BGProcessingTask` | 🟡 | 面向长任务（可几分钟），但要求设备空闲/充电，频率更低，且需要 `processing` 后台模式 |
| 后台 URLSession（`URLSession` background） | ❌ 不适用 | 只能跑下载上传，不能跑业务逻辑 |
| 静默推送（`content-available`） | 🟡 | 需要**自建推送服务器**；iOS 会限流，低电量模式直接丢弃；App 被杀后仍能唤醒但**不保证** |
| **本地通知** | ❌ | 「本地」= 手机自己发，**不会唤醒 App**。想靠推送唤醒必须走远程推送 |
| VoIP 推送（PushKit + CallKit） | ❌ **不要用** | iOS 13 起收到 VoIP 推送必须立即 `reportNewIncomingCall`，否则**系统直接杀掉进程**；滥用会被 App Store 拒审，自签也可能被系统惩罚 |
| **`NEPacketTunnelProvider`（VPN 扩展）** | ✅ **唯一可行** | 这是 iOS 上唯一能**长期驻留后台**的网络机制（代理类 App 都靠它）。代价：需**付费开发者账号**（688 元/年）+ Network Extension entitlement；必须伪装成真实 VPN 隧道；自签免费账号**拿不到这个 entitlement**；上架需额外审核 |

### 2.3 所以「保活」到底能做到什么

**结论：做不到安卓那种常驻。** 能选的三条现实路径，按代价从低到高：

1. **接受「不保活」，用 BGAppRefreshTask 尽力而为**
   —— 见 `app/BackgroundRefresh.swift`。App 被使用得越频繁，系统给的机会越多。
   典型表现：一天能跑几次到十几次，早上第一次大概率能跑到。**能补登录，但不能保证及时**。
   配合「本地通知」在补登录成功后提示用户，至少不会静默失败。
2. **自建推送服务器 + 静默推送**（进阶）
   —— 自己租一台 VPS 跑定时任务，到点发 `content-available` 推送唤醒 App 去登录。
   这是最接近「可控定时」的方案，但**要维护服务器、要付费开发者账号（免费号不能推送）、要处理 APNs 证书**。
   对「校园网自动登录」这个需求来说，投入产出比很差。
3. **VPN 扩展形态**（最接近安卓，代价最高）
   —— 用 `NEPacketTunnelProvider` 让 App 常驻，在隧道内定时探测并重登。
   真能做到接近秒级，但等于为了自动登录去写一个代理 App，且必须付费账号。

> 一句话：**iOS 上「掉线主动重连」不是代码问题，是权限模型问题**。
> 换任何第三方 App（包括商业 VPN）都绕不开上面三条路之一。

---

## 3. 签名与分发

（成本与路径的完整对比见 [ipa-roadmap.md](ipa-roadmap.md)，这里只列决策要点。）

### 3.1 账号类型

| 账号 | 费用 | 有效期 | 能否用推送 | 能否用 Network Extension | 设备数 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| 免费 Apple ID | 0 | **7 天**，需重签 | ❌ | ❌ | 约 3 台 |
| Apple Developer Program（个人） | **688 元 / 年** | 1 年证书 | ✅ | ✅ | 100 台 / 年 |
| 企业账号（Enterprise） | 299 美元 / 年 | 1 年 | ✅ | ✅ | 内部员工，**申请门槛高，滥用会被封** |

> 只做「登录 / 状态 / 断开」三件事 → **免费账号 + AltStore 就够**，不需要付费。
> 想要 BGAppRefreshTask 之外的保活 → 必须付费。

### 3.2 分发路径

| 路径 | 成本 | 维护 | 适用 |
| :--- | :--- | :--- | :--- |
| **AltStore / SideStore 自签** | 0 | 每 7 天自动/手动重签，Windows 上常驻 AltServer | **个人自用，推荐** |
| 云 Mac 构建 + 免费证书直装 | 0 | 每 7 天手动重来一次 | 只想验证一次 |
| **TestFlight** | 688 元/年 | 90 天一个构建，发版即可 | 分发给同学（≤100 人内部测试） |
| App Store 上架 | 688 元/年 | 审核 | ❌ **过不了**（后台常驻类目会被拒） |
| Ad Hoc 分发 | 688 元/年 | 需收集设备 UDID，100 台上限 | 小范围固定人群 |

### 3.3 无 Mac 时的构建

- **Codemagic**：免费额度约 500 分钟/月，对 Swift 项目开箱即用。
- **GitHub Actions**（`macos-latest` runner）：公共仓库免费，私有仓库有额度。
- 二者都能产出未签名 `.ipa` 或已签名产物；自签还需在本地用 AltServer / Sideloadly 注入证书。

---

## 4. 与快捷指令方案的关系

| 场景 | 推荐 |
| :--- | :--- |
| 只想「连上校园网就自动登录」 | **快捷指令**（零成本、零维护、不会 7 天过期） |
| 想要原生界面、状态常显、后台尽力补登录 | **App**（接受 7 天重签 / 688 元） |
| 想要秒级掉线自愈 | **iOS 做不到**，用 Windows / Linux / Android 端 |

两条路线可以并存：App 装上后用 App，快捷指令留作备份。

---

## 5. 骨架代码与自查声明

目录：`ios/app/`

| 文件 | 作用 |
| :--- | :--- |
| `PortalClient.swift` | 对齐 `portal.rs`：5 字段表单 POST、11 端口 failover、状态页解析、logoff |
| `KeychainStore.swift` | 密码存 Keychain（而非 UserDefaults） |
| `PortalViewModel.swift` | 界面状态与动作编排 |
| `ContentView.swift` | 登录 / 状态 / 断开三按钮 |
| `BackgroundRefresh.swift` | `BGAppRefreshTask` 注册、续排、补登录 |
| `SNNUAutoLoginApp.swift` | App 入口，启动时注册后台任务 |
| `Info.plist` | 后台模式、ATS 明文放行、任务标识 |

### 自查声明（重要）

**本机没有 Swift 工具链**（`swift` / `swiftc` 均不存在，也无 Xcode），因此：

- ❌ **未编译**，未在模拟器或真机运行
- ✅ 已做**人工静态自查**：API 名称与签名、`async/await` 用法、`actor` 隔离、
  `@MainActor` 标注、SwiftUI 属性包装器配对、Keychain 字典键类型、`Info.plist` 键名
- ⚠️ 首次在 Xcode 打开时，仍可能需要处理：签名 Team 配置、`Info.plist` 合并方式（若用 Xcode 15+ 的
  `GENERATE_INFOPLIST_FILE`，需把这里的键挪进 Build Settings）、部署目标（代码按 **iOS 16+** 编写）

> 部署目标定为 iOS 16：用到了 `NavigationStack` 之外的通用 API 与 `async/await` 的 `URLSession`，
> iOS 15 也能编过，但 16+ 更稳。

---

## 6. 明确不做的事

- ❌ 不试图「主动连接 / 切换 Wi-Fi」——无公开 API
- ❌ 不试图绕过 7 天重签——免费账号的硬限制
- ❌ 不使用 VoIP 推送做保活——会被系统杀进程 + 拒审
- ❌ 不把「掉线秒级自愈」写进任何宣传文案——做不到就是做不到
