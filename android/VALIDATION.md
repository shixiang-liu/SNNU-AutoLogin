# Android 验证记录

2026-11-01（Asia/Shanghai），当前版本 **1.1.0 / versionCode 3**。开发测试签名版；真实校园网已验证，厂商自己的电池开关只能以系统页面为准。

> 版本号从 1.0.0 升到 1.1.0 与桌面版对齐，versionCode 由 2 改为 3（保持单调，可直接覆盖安装旧测试包）。
> debug 变体不再加 `.debug` / `-test` 后缀，产物包名就是 `io.github.shixiangliu.snnulogin`；
> 代价是测试包与正式包不能再共存于同一台设备。

## 当前版本已通过

| 检查 | 结果 |
| --- | --- |
| 协议、账号保护、状态呈现、调度与脱敏 | 85 项离线测试通过，0 失败 |
| Debug / 优化测试包 / Release 构建与 Lint | 构建通过；Lint 0 错误、3 条已说明的警告；未提供密钥时 Release 为未签名产物 |
| 真机（PHB110，Android 16 / API 36） | 9/9 设备检查通过 |
| Android 8 模拟器（API 26 / x86_64 AOSP） | 9/9 设备检查通过 |
| 大字体与窄屏 | 1.8 倍字体、320 × 568 dp 下不重叠、不裁切，长文案换行正常 |
| 正式签名包 | `SNNU-AutoLogin-android.apk`（1.0.5，包名 `io.github.shixiangliu.snnulogin`）通过 `apksigner verify`，在 API 26 模拟器安装并启动，页面可正常切换 |

真机端到端（一加 11，学校 Wi-Fi）：

| 场景 | 结果 |
| --- | --- |
| 连接 | 首页进入「已连接」，账号与上网方式取自门户会话；`ping` 0% 丢包、HTTP 200 |
| 断开 | 回到「自动连接已暂停」，账号显示为「已保存账号」 |
| 重连 | 恢复「已连接」，外网恢复 |
| 关闭移动数据后仅用 Wi-Fi | 状态读取、认证与上网全部正常，门户路由确认走 `wlan0` |
| 冷启动（服务未运行） | 只读读取当前状态，直接显示「已连接」，不再出现「准备连接」 |
| 强制停止后重开 | 状态与在线时长正确，未重复认证也未误报掉线 |
| 关闭再打开 Wi-Fi | 依次显示「等待学校 Wi-Fi」→ 自动重连「已连接」，无需手动操作 |
| 点击「重新检测」 | 立即出现「正在重新读取认证状态…」，结果刷新后消失 |

## 本轮修正

- **状态优先级**：学校门户的回答现在优先于系统对 Wi-Fi 的「可以上网」判断。此前未登录时会先命中系统判断，显示「网络可用」，点重新检测也返回同一状态，看起来像没反应。现在未登录就显示未登录或已暂停。
- **去掉厂商专用文案**：界面里不再出现 ColorOS 或 App battery usage 之类字样，所有安卓设备使用同一套说明；「打开应用设置」统一跳到应用详情页，厂商的电池使用设置在那里。
- **连接诊断**：改为逐行信息（网络名称、账号、上网方式、IP、在线时长、认证地址）。网络名称不再硬编码为「校园网 Wi-Fi」——Android 12 起未获定位授权读不到 SSID，此时显示 —。
- 上一轮已修：脱敏把 `示例学号@mobile` 显示成 `示例***le`；账号卡在名称不可读时残留的裸「Wi-Fi」。
- 检测节奏改为按可见性分档：前台可见 **5 秒**、后台在线 **30 秒**、后台读不到门户 **20 秒**；Wi-Fi 变化、系统「可上网」状态翻转、亮屏、打开应用都会立即触发一次检查。连接诊断新增「上次检测」，可看到状态确实在刷新。
- 修复前台服务启动超时崩溃：`startForegroundService()` 后若 5 秒内未 `startForeground()`，系统会杀掉进程（本机 16:20 出现过一次 `ForegroundServiceDidNotStartInTimeException`）。现在 `onCreate` 一进就先贴一条最小通知进入前台，再做磁盘与 Keystore 初始化。事后连续强制停止/重启三轮未再复现。
- 门户读不到时文案写明「正在自动重试」，圆形按钮的检测动作改为「立即检测」。
- 连接诊断的网络名称固定为 **SNNU**（工具只服务学校这一张网，不读 SSID，也就不需要定位权限），首页账号卡同步显示「SNNU · 已在线 …」。
- 子页面标题栏换成圆形返回按钮（箭头 + 1 dp 描边 + 按下涟漪），替代原来的纯文字「返回」。
- 去掉「停止后台运行」这个独立开关：关闭「自动登录」就等于停掉前台服务与通知，当前认证连接保留；重新打开应用时若自动登录仍为开启状态，会立即恢复后台。
- 设置页底部只显示居中版本号，不再混排「社区开源工具」等字样。
- 测试包与正式版包名不同，系统后台限制按包名保存；被限制时页面写出当前包名。

## 体积与内存

正式包 **56,516 字节（约 55.2 KiB）**，测试包 56,636 字节，低于 2 MiB 预算。校验值见 `dist/android/SHA256SUMS.txt`。运行时没有 native `.so`、WebView 或第三方 UI 引擎。

真机（一加 11 / Android 16）实测，每组 30 次采样、每 2 秒一次：

| 场景 | PSS 中位数 | 最大值 |
| --- | --- | --- |
| 前台（页面可见） | 96.9 MiB | 97.3 MiB |
| 后台（前台服务在线） | 23.9 MiB | 24.3 MiB |

后台 **30 MiB 目标达成**。原始记录位于 `dist/android/measurements/*20261027-16*.json`，包含机型、系统与全部采样值。更早的 1.0.2 模拟器基线不代表当前版本。

## 尚待真机验证

- 厂商自有电池开关是否放行后台：应用读不到，只能以系统页面为准。当前测试包已获 Android 省电豁免，系统后台限制为允许。
- 锁屏、Doze、重启首次解锁后的恢复，以及长时间耗电；厂商自启动开关同样无法读取。
- 欠费、套餐超限、验证码等真实账号异常场景，以及其他品牌机型。

## Wi-Fi 自愈（本轮新增，待 CI 与真机验收）

2026-11-01，分支 `android-release`。与 Windows 版 `src/wifi_heal_core.rs` 的 `decide()` 与 `internet_gate()` 对齐：**只有「连接的网络不可用」才自愈**，连着可用网络（校园 Wi-Fi 或手机自己的蜂窝）时绝不强求连 SNNU。决策是纯 Java（`WifiSelfHeal`），系统调用单独一层（`WifiHealer`）。

挂点两处，都在 `LoginService.tick()`：

| 设备此刻的状态 | 判断依据 | 结论 |
| --- | --- | --- |
| 连着某个 Wi-Fi，门户不可达 | 经**这张 Wi-Fi** 的外网探测（`WifiTransport` 绑定该网络） | 外网通 → 不动作（`INTERNET_UP`）；已连 SNNU → 不动作（`ON_TARGET_SSID`）；都不通且不是 SNNU → 请求连接 |
| 没有任何 Wi-Fi 关联 | 手机自己的出口（`getActiveNetwork`，非 Wi-Fi 传输且已 `VALIDATED`） | 有蜂窝 → 不动作（`MOBILE_NETWORK`，用户在用流量）；一点出口都没有 → 请求连接学校网络 |

与桌面的平台差异：桌面「没有任何关联 + 外网通」只能来自 TUN/虚拟网卡，所以 `internet_gate` 判定为「不在校园网里」、继续尝试连回；手机「没有任何关联 + 外网通」是用户自己开的蜂窝数据，判定相反——原地不动。两处都写进 `WifiSelfHeal.decide` 的注释里。

**本轮修正**：此前没有无线关联时 `tick()` 直接 `return`，只显示「请先连接学校 Wi-Fi」，一次自愈尝试都不做，与桌面「没有任何关联时继续尝试连回」不一致。现在该分支也会判一次自愈。

| 检查 | 结果 |
| --- | --- |
| 纯决策类 + 离线 JUnit | `WifiSelfHealTest` **15 项通过、0 失败**（javac + JUnit 4.13.2，**未加载 android.jar**，证明决策类没有 Android 依赖） |
| 既有离线测试零回归 | 合计 **100 项**复跑全过：WifiSelfHeal 15、ConnectionPolicy 6、PortalProtocol 17、PortalClient 14、AccountUpdate 10、ConnectionInspection 3、UiModel 29、SystemSettingsState 6 |
| Android 组装层可编译 | `WifiHealer` 与 `LoginService` 的改动用 android.jar（API 36 平台桩）+ 最小 R/BuildConfig 桩编译通过 |
| 真机构建 | **本机已通过**：`gradlew clean assembleDebug` 成功，产物 `dist/SNNU-AutoLogin-v1.1.0-android-debug.apk`（90,129 字节），`aapt dump badging` 确认包名 `io.github.shixiangliu.snnulogin`、versionName 1.1.0、versionCode 3、minSdk 26、targetSdk 37，`apksigner verify` 为 Android Debug 证书 |
| Lint 与设备矩阵 | **本机未验证**：`lintDebug`、`assembleBenchmark` 与 API 26/33–37 模拟器矩阵交由 `.github/workflows/android.yml` 验收 |
| 真实 Wi-Fi 切换 | 开发期间**未做任何真实连接或断网动作**，连接调用只以打桩与静态审查为准 |

新增清单权限 `CHANGE_WIFI_STATE`（正常权限，无运行时弹窗）；未申请定位权限，也未申请 `NEARBY_WIFI_DEVICES`。

尚待验收：

- Lint 是否接受新增权限、以及 `WifiConfiguration`（API 29 起弃用）这条旧路径；两处 API 级别判断都写在方法内部。
- API 26–28 的旧接口路径与 API 29+ 的系统建议路径在真机上的实际效果：系统可能延迟或忽略建议，用户也能在系统设置里撤销。
- 「无无线关联 + 无蜂窝」时请求连接 SNNU 的真实效果：`addNetworkSuggestions` 在没有任何 Wi-Fi 关联时是否会被系统采纳，只能真机确认。
- 未申请定位权限时 `WifiInfo.getSSID()` 的返回（官方文档为 `<unknown ssid>`）：此时跳过「已连学校网络」判断并照常尝试连接，需要一次真机确认。
