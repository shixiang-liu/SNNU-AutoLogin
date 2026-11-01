# Android 开发与验证

本页面向从源码构建、调试或贡献代码的开发者。下载安装与日常使用见 [Android 使用指南](../android/README.md)。

客户端使用 Java、系统控件、系统网络接口和单个前台服务，没有第三方运行时依赖。支持 Android 8.0 及以上，各客户端独立保存账号。

## 功能对应

| Windows 功能 | 安卓实现 |
| --- | --- |
| 登录、下线及校园网/移动/联通 | 原生 HTTP 认证，支持保存与在线切换设置 |
| 自动登录、掉线重连、保活 | 网络事件触发与定时检查，单个前台服务 |
| 失败退避、验证码限制、假在线修复 | 对应错误分类、熔断和冷却逻辑；保护其他账号 |
| 门户地址识别 | 自动发现与验证；陌生主机需用户确认 |
| DPAPI 加密密码 | Android Keystore + AES-GCM |
| Wi-Fi 自愈 | 同一套三条件门控：外网不通 + 门户不可达 + 不在 SNNU 上；走 `WifiNetworkSuggestion`（API 29+）或旧 `WifiConfiguration` 接口（API 26–28） |
| 托盘操作 | 常驻通知与操作按钮 |
| 开机运行 | 首次解锁后尝试恢复，默认关闭，受系统和厂商限制 |
| 网络信息与诊断导出 | 应用内查看和系统文件选择器导出脱敏报告 |

安卓版目前处理 Wi-Fi 校园网认证。连接、断开与重连已在一加 11 上完成真实校园网验证；其他机型与厂商后台限制仍需验收，具体状态见验证记录。

只处理校园网认证，不代替系统连接 Wi-Fi，不使用 VPN、Root、无障碍服务、精确闹钟或常驻唤醒锁。所有认证和联网检测使用指定 Wi-Fi，不回退到移动数据。不要求 Wi-Fi 已被 Android 标记为可上网。

连接诊断按行显示网络名称、账号、上网方式、IP、在线时长与认证地址。界面上的网络名称固定显示 SNNU——本工具只服务学校这一张无线网，不去猜别的网络名。

需要区分两件事：界面文案是写死的，而 Wi-Fi 自愈会**尝试读取系统 SSID**（`WifiHealer.currentSsid()`，需要 `CHANGE_WIFI_STATE`）。Android 12 起读取 SSID 还要求定位权限与系统定位开关，本应用**不申请定位权限**，因此这条读取在真机上通常拿不到值，系统返回 `UNKNOWN_SSID`。此时自愈不会跳过判断，而是按保守语义继续尝试连接（与 Windows 读不到 SSID 时一致），只在日志里留一行。代价与取舍理由、以及尚待真机确认的验收项，见 [Android 验证记录](../android/VALIDATION.md) 的 Wi-Fi 自愈小节与「未申请定位权限时 `WifiInfo.getSSID()` 的返回」一条。

## 后台调度与系统限制

检测节奏分三档，按“看不看屏幕”而不是按固定间隔：

| 场景 | 间隔 | 说明 |
| --- | --- | --- |
| 应用在前台可见 | 5 秒 | 只读查询认证页，不提交账号；界面里的「上次检测」会跟着更新 |
| 后台在线 | 30 秒 | 前台服务轮询，兼顾掉线恢复与耗电 |
| 后台读不到门户 | 20 秒 | 门户临时异常时尽快恢复 |

除定时轮询外，Wi-Fi 连接/断开、Android 的「可上网」状态翻转（门户出现或消失）、亮屏、打开应用都会立即触发一次检查。自动连接开启时另有约 2 分钟一次的外网诊断，用于确认假在线并在连续失败后尝试恢复。

关闭「自动登录」后，不再发送额外的外网检测请求，也不自动提交认证或修复连接；仍可读取校园网认证状态以显示当前连接。原有的暂停后联网检查开关已移除，旧配置不再影响运行。

Android 14 及以上声明 `specialUse` 前台服务及其校园网认证用途，不使用有时长限制的 `dataSync` 类型冒充常驻后台。前台服务不等于无限制运行：系统休眠可能延迟联网，厂商省电和自启动设置也可能限制后台。恢复运行后继续检查，不承诺所有手机锁屏期间即时重连。拒绝通知权限不改变 Android 对前台服务的要求，通知栏快捷操作可能不可见。

需要持续重连时，在「设置 → 后台与通知」中打开系统电池优化和应用设置，按实际需要允许后台和自启动。强行停止应用后需手动重新打开。账号错误、欠费、验证码等问题暂缓自动提交 30 分钟，点击连接或重新保存账号可以主动重试。验证码持续出现时，通过学校认证页面处理。

## 数据与认证边界

- 密码通过 Android Keystore 的 AES-256-GCM 密钥加密后保存在私有 SharedPreferences，配置和日志禁止备份、设备迁移。密钥不可用时要求重新输入，不保存明文作为兜底。
- 密码输入不进入界面状态备份或自动填充。
- 学校门户使用 HTTP。存储加密不等于传输加密。
- 默认信任现有门户 `202.117.144.205` 和 `snnu.edu.cn` 域名下的门户；登录前重新验证状态页，禁止跨地址自动携带密码或 Cookie 跳转。
- 发现其他新主机时，先验证门户结构和学校页面标识，并在「设置 → 连接诊断 → 核对新的认证地址」中由用户确认后才提交密码。HTTP 页面标识不能验证主机身份，请核对学校公布的地址。
- 网页已经登录其他账号或其他上网方式时，保留会话；自动修复仅处理本次后台认证成功且账号和上网方式相符的会话，并设 20 分钟修复冷却。
- 联网检测访问小米、微软公开检测地址，不携带校园网账号、密码或会话 Cookie。
- 私有日志超过 200 KiB 轮转，保留一份旧日志。诊断报告脱敏账号和密码，包含机型、门户和可能的 IP，分享前请检查。

## 构建

需要 JDK 21（最低 17）、Android SDK Platform 37.0 和 Build Tools 36.0.0。Gradle Wrapper 固定 9.3.1，并校验官方分发包 SHA-256；Android Gradle Plugin 固定 9.1.1。不需要 Android Studio，可以只安装命令行 SDK。

配置 `ANDROID_HOME`，或在未纳入版本管理的 `android/local.properties` 设置 `sdk.dir`。

```powershell
android sdk install 'platforms;android-37.0'
android sdk install 'build-tools;36.0.0'
android sdk install 'platform-tools'
./android/gradlew.bat -p android testDebugUnitTest lintDebug lintBenchmark assembleBenchmark assembleDebugAndroidTest
./scripts/android-package.ps1
```

Linux/macOS 使用 `bash android/gradlew -p android ...`。运行时无第三方依赖，测试时使用 JUnit 4.13.2；设备冒烟测试使用平台 Instrumentation。

上面的 `android` 是新版 Android CLI。旧版命令行工具也可以使用 `sdkmanager 'platforms;android-37.0' 'build-tools;36.0.0' 'platform-tools'`。需使用包含 `javac` 和 `jlink` 的完整 JDK；某些 IDE 内置运行时缺少 `jlink`。Windows 打包脚本会在当前 `JAVA_HOME` 不完整时尝试使用用户 `.jdks` 下的完整 JDK。

构建类型：

- `debug`：`.debug` 包名，调试签名，用于设备自动化测试。
- `benchmark`：`.benchmark` 包名，优化和资源裁剪已开启，调试签名，供安装测试和性能测量。
- `release`：正式包名。未提供正式签名时仅生成未签名产物，不会偷偷使用测试签名发布。

正式签名通过 `SNNU_KEYSTORE`、`SNNU_STORE_PASSWORD`、`SNNU_KEY_ALIAS`、`SNNU_KEY_PASSWORD` 环境变量注入，四项必须一起提供。签名文件、密码不进入仓库。

```powershell
# 在本机安全地配置上述四个变量后：
./scripts/android-package.ps1 -Release
```

GitHub Actions 自动构建和上传测试 APK，模拟器矩阵为 API 26、31、33、34、35、36、37。正式签名任务只在手动触发并勾选 `sign_release` 时执行，使用 `android-release` environment 的上述密码 secrets，以及保存为 Base64 的 `SNNU_KEYSTORE_BASE64`；它上传产物，不自动发布 Release。

## 验证与性能

离线单元测试注入 HTTP 传输，覆盖字符编码、Cookie、跳转、字段编码、验证码、熔断、账号保护及取消竞争，不向学校提交账号。设备测试只对干净的 `.debug` 包运行，检查原生界面、Keystore 加密及前台服务启停。

```powershell
./android/gradlew.bat -p android connectedDebugAndroidTest
adb install -r dist/android/SNNU-AutoLogin-android-test.apk
# 在应用中启用后台，并保持待测连接状态：
./scripts/android-measure.ps1 -Adb '你的SDK/platform-tools/adb.exe' -Scenario Background
```

性能目标：优化 APK 不超过 **2 MiB（2,097,152 字节）**，后台 PSS 目标 **30 MiB**。内存脚本在约 60 秒内采样 30 次，每 2 秒一次，记录机型、系统、最小/中位/最大 PSS。前台、后台在线、等待校园网分别测量；等待状态由测试者手动准备，脚本不切换 Wi-Fi。真实在线状态和耗电必须在校园网真机验证，不能以模拟器结果替代。

验证记录见 [验证记录](../android/VALIDATION.md)。构建结果与测试报告位于 `app/build/`，打包结果位于仓库 `dist/android/`，均不纳入源码。
