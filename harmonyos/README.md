# 陕Star校园网自动登录 · HarmonyOS NEXT 客户端

HarmonyOS NEXT（API 12+，Stage 模型，ArkTS / ArkUI 声明式开发）版本的校园网门户客户端。
功能面与上游 Android 1.0.0 的 `LoginService` 对齐：**登录 / 状态 / 断开 + 应用存活期间的掉线重连**。

> **移动端原则**：手机端**不做 Wi-Fi 自愈**。连接哪个 Wi-Fi 由系统自动化接管，
> 本应用只负责校园网认证本身。这条原则同时写进了 `PortalClient.ets` 与 `PortalService.ets` 的文件头注释。

---

## 1. 工程结构

```
harmonyos/
├── AppScope/
│   ├── app.json5                          # bundleName com.pursuec.snnulogin, versionCode 1 / versionName 1.0.0
│   └── resources/base/
│       ├── element/string.json            # 应用名
│       └── media/app_icon.png             # 应用图标
├── build-profile.json5                    # products / modules / buildModeSet
├── hvigorfile.ts                          # appTasks
├── hvigor/hvigor-config.json5
├── oh-package.json5
└── entry/
    ├── build-profile.json5                # stageMode
    ├── hvigorfile.ts                      # hapTasks
    ├── oh-package.json5
    ├── obfuscation-rules.txt
    └── src/main/
        ├── module.json5                   # EntryAbility + INTERNET / GET_NETWORK_INFO / KEEP_BACKGROUND_RUNNING
        ├── ets/
        │   ├── entryability/EntryAbility.ets
        │   ├── pages/Index.ets            # 学号/密码/运营商 + 登录/状态/断开 + 状态区
        │   └── portal/
        │       ├── PortalClient.ets       # 协议层：HTTP / 表单 / 11 端口 failover / 解析
        │       └── PortalService.ets      # 长时任务 + 15s 轮询 + 掉线重连 + 通知
        └── resources/
            ├── base/element/{string,color}.json
            ├── base/media/{app_icon,startIcon,background,foreground}.png
            ├── base/media/layered_image.json
            ├── base/profile/main_pages.json
            └── en_US/element/string.json
```

## 2. 运行步骤

### 2.1 安装 DevEco Studio

1. 打开华为开发者官网下载页：<https://developer.huawei.com/consumer/cn/download/>
2. 选择 **DevEco Studio 5.x（HarmonyOS NEXT）** 的 Windows / macOS 版本，下载安装包。
3. 安装过程中保持默认组件（含 HarmonyOS SDK、hvigor、hdc 命令行工具）。
4. 首次启动按向导完成 **SDK 下载**（需要 HarmonyOS NEXT SDK API 12 及以上）。

> 官网下载需要登录华为账号；未实名的账号可以下载工具，但**无法真机运行**（见 2.3）。

### 2.2 打开工程

1. DevEco Studio → **File → Open**，选择本仓库的 `harmonyos/` 目录（**不是仓库根目录**）。
2. 等待 hvigor 同步完成（右下角进度条消失、`entry` 模块出现绿色图标）。
3. 若提示「无法找到 API version 26.0.0 的 SDK」：DevEco 首次启动尚未下载对应 SDK。**File → Settings → HarmonyOS SDK**（或欢迎页 More Actions → SDK Manager）→ 勾选 **API 26 / HarmonyOS 7.0** 的 SDK 与 Toolchains → Apply 等下载完成后重新同步即可。工程已按 API 26 声明，装好后即可识别。

### 2.3 签名配置（真机运行的前置条件）

1. 右上角 **File → Project Structure → Signing Configs**。
2. 勾选 **Automatically generate signature**。
3. 按提示登录**华为账号**并完成**实名认证**（未实名的账号无法申请调试证书）。
4. 等待 DevEco Studio 自动申请调试证书与 Profile，成功后 `build-profile.json5` 的 `signingConfigs` 会被自动填充。

> 本仓库提交的 `build-profile.json5` 里 `signingConfigs` 是空数组 —— 签名与账号强绑定，不能共享。

### 2.4 真机运行

1. 手机开启开发者模式：**设置 → 关于手机 → 连续点击「版本号」7 次**。
2. 打开 USB 调试：**设置 → 系统和更新 → 开发人员选项 → USB 调试**。
3. 数据线连接电脑，手机上确认「允许 USB 调试」。
4. DevEco Studio 顶部设备下拉框出现机型后，点 **Run ▶**（或 `Shift+F10`）。
5. 首次启动会申请**通知**权限（长时任务的常驻通知需要它），请允许。

## 3. 权限与「长时任务」的真机限制

`module.json5` 声明了三个权限：

| 权限 | 用途 | 是否需要用户授权 |
| :--- | :--- | :--- |
| `ohos.permission.INTERNET` | 访问校园网门户 | 否，安装即生效 |
| `ohos.permission.GET_NETWORK_INFO` | 读取网络连接信息 | 否，安装即生效 |
| `ohos.permission.KEEP_BACKGROUND_RUNNING` | 申请长时任务，退到后台后继续轮询 | 否，但**长时任务本身受系统策略约束** |

**关于「长时任务要不要在 AGC 申请」——如实说明**：

- `KEEP_BACKGROUND_RUNNING` 属于 **normal 级权限**，**不需要**在 AppGallery Connect 单独提交权限申请，也不需要填写使用理由审批表。
- 但 `backgroundTaskManager.startBackgroundRunning` 的**持续时长由系统管控**：应用被用户手动清理、系统内存紧张、或厂商省电策略介入时，长时任务会被回收，轮询随之停止。
- 上架（发布到应用市场）时，长时任务的使用场景会在**隐私标签 / 权限说明**里被审核，需要如实填写「校园网认证状态保持」；调试证书（debug）不涉及这一审核。
- 代码已做降级：`startBackgroundRunning` 失败时只打一条 `console.warn`，前台轮询继续，登录功能不受影响（见 `PortalService.ets` 的 `startContinuousTask`）。

## 4. 协议对齐

本客户端与桌面端 `src/portal.rs` 的协议实现逐字对齐，详见 [协议对齐说明](PROTOCOL.md)。

## 5. 常见问题

### hdc 找不到设备

```
hdc list targets      # 输出 [Empty] 表示没有识别到设备
```

- 换一根**支持数据传输**的线（很多随机附带的线只供电）。
- 手机端把 USB 连接方式从「仅充电」改成「传输文件」。
- Windows 需要装华为手机驱动（DevEco Studio 安装目录 `tools\hdc\` 下有 `hdc.exe`，可先手动执行 `hdc start`）。
- 关闭其他占用 adb/hdc 端口的程序（Android Studio、投屏工具）。
- 若列表里出现 `unauthorized`：在手机上重新确认 USB 调试授权弹窗，或「撤销 USB 调试授权」后重插。

### 签名失败

- `Failed to generate signature` / `Profile is invalid`：**File → Project Structure → Signing Configs** 里取消勾选再重新勾选 *Automatically generate signature*，重新登录华为账号。
- 提示「账号未实名」：去 <https://developer.huawei.com/consumer/cn/> 完成实名认证（个人认证 1～2 个工作日）。
- 提示「设备未加入调试设备列表」：AGC 的调试 Profile 里最多登记 100 台设备，先在 Project Structure 里删除旧 Profile 再重新生成。

### 证书过期

- 调试证书有效期 **90 天**，到期后安装会报 `install failed due to grant request permissions failed` 或 `certificate expired`。
- 处理：**File → Project Structure → Signing Configs** 取消勾选 → 重新勾选 *Automatically generate signature*，DevEco Studio 会重新签发证书。
- 已经装在手机上的旧包需要先卸载再装新包。

### 登录一直提示「连不上认证服务器」

- 确认手机已连上 **SNNU** 校园网（本应用不负责连 Wi-Fi）。
- 门户 11 个候选端口会依次尝试，全部不可达才会报这句；此时多半是根本没连到校园网，或门户临时维护。
- 校园网之外的网络访问该 IP 会超时，属正常现象。

### 断开后又被自动登回去

- 断开成功会**自动停止轮询**；如果关掉开关后仍被重连，检查状态区是否显示「自动重连已停止」。
- 手动点「断开」时若门户没有返回离线状态，应用会提示「断开未确认」，此时轮询不会停止，避免误判。

## 6. 与桌面端的差异

| 能力 | 桌面端（src/portal.rs） | HarmonyOS 端 |
| :--- | :--- | :--- |
| Wi-Fi 自愈 | 有（Windows / macOS / Linux） | **无**（移动端原则：交给系统自动化） |
| 凭据存储 | 系统钥匙串 / DPAPI 加密落盘 | 仅驻留内存，退出应用即丢弃 |
| 轮询间隔 | 事件驱动 + 心跳 | 固定 15s（前台态） |
| 后台保活 | 托盘常驻 | 长时任务（受系统策略约束） |
| 协议 | 11 端口 failover / 5 字段表单 / 同源跳转限制 | 完全一致 |
