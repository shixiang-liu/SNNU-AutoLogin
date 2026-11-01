# macOS 使用指南

当前版本 **v1.0.0**。支持 **macOS 13 及以上**，提供 Apple Silicon（arm64）与 Intel（x64）两个安装包。

[下载 macOS v1.0.0](https://github.com/shixiang-liu/SNNU-AutoLogin/releases/tag/macos-v1.0.0) · [返回项目首页](../README.md) · [从源码构建](building.md#macos)

## 第一次使用

1. 从发布页下载对应安装包：M 系列芯片选择 `macos-arm64.zip`，Intel 芯片选择 `macos-x64.zip`。
2. 解压，将「校园网自动登录.app」移到「应用程序」，连接学校 Wi-Fi 或有线网络。
3. 打开应用，填写学号、校园网密码和上网方式，点击「保存并连接」。

当前构建没有 Apple Developer ID 签名与公证。首次打开若被系统拦截，请先确认下载来源，再按 [Apple 官方说明](https://support.apple.com/102445)，在「系统设置 → 隐私与安全性」中选择「仍要打开」。

## 界面预览

| 首页 | 设置 |
| :---: | :---: |
| <img src="https://raw.githubusercontent.com/shixiang-liu/SNNU-AutoLogin/main/docs/images/macos/home.png" width="300" alt="macOS 原生首页"> | <img src="https://raw.githubusercontent.com/shixiang-liu/SNNU-AutoLogin/main/docs/images/macos/settings.png" width="300" alt="macOS 设置"> |

截图来自自动化验证的模拟会话，不包含真实校园网账号。

## 体积与内存

| 架构 | App 体积 | 在线后台物理占用 |
| :--- | ---: | ---: |
| Apple Silicon | 1.21 MiB | 13.39 MiB |
| Intel | 1.25 MiB | 9.70 MiB |

以上为 macOS 15.7.9 云端模拟门户测量，详见 [完整性能记录](performance.md)。

## 日常操作

| 操作 | 会发生什么 |
| :--- | :--- |
| 连接 | 完成校园网认证，并开启自动登录 |
| 断开 | 退出校园网认证并关闭自动登录；Wi-Fi 保持连接 |
| 关闭「自动登录」 | 停止自动认证与重连，保留当前连接 |
| 关闭窗口 | 应用继续在菜单栏运行 |
| 从菜单栏退出 | 退出应用，保留当前网络连接 |

## Wi-Fi 自愈

本身没网、校园网门户也不可达、又没连在校园网里时，应用会尝试把 Wi-Fi 连回学校网络。三个条件缺一不可：外网还能上（只是门户自己故障）时绝不动 Wi-Fi，有线在线或已经连在目标 Wi-Fi 上时也不干预。只连接系统里已经保存过的网络，不新建、不修改任何系统配置。

| 配置项 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `wifi_self_heal` | `true` | 关闭后完全不做自愈 |
| `ssid` | `SNNU` | 自愈目标网络名称，留空按默认学校网络 |

两项都在 `~/Library/Application Support/SNNU-AutoLogin/config.json` 里，与 Windows 端同一套字段名；账号保存时会一并写回。

macOS 侧的实现与限制：

- 有线判定读 `networksetup -listallhardwareports` 里的以太网类端口，再看该设备 `ifconfig` 的链路状态；
- 当前网络优先读 `airport -I`；`airport` 在 macOS 14.4 起已被移除，此时回退 `networksetup -getairportnetwork`；
- 附近网络扫描同样依赖 `airport -s`，扫描不可用时不再扫描，改用两次尝试之间至少 60 秒的冷却限频；
- 连接使用 `networksetup -connectairport`（开放网络免密）。部分 macOS 版本没有这个子命令，此时应用只记日志、不做连接动作，绝不会退回会新建系统网络配置的 `-setairportnetwork`。

以上命令都没有在开发机上真实执行过（开发机是 Windows，无法切换 Wi-Fi），macOS CI 与实机验证仍是最终依据。

## 系统权限

- **本地网络**：用于访问学校认证服务器，请允许。
- **登录项**：开启登录自启动后，可能需要在「通用 → 登录项」中批准。
- **通知与定位**：通知用于提示连接状态，定位仅用于显示 Wi-Fi 名称；拒绝时仍可使用认证功能。

密码保存在系统钥匙串中。若访问被拒绝，请解锁或允许访问，再通过应用菜单重新读取；更新构建后，系统可能再次询问权限。

## 数据与卸载

配置和日志保存在 `~/Library/Application Support/SNNU-AutoLogin/`，密码单独保存在钥匙串中，各客户端的密码互不迁移。

卸载前可先在设置中删除账号、关闭登录自启动，再退出并删除应用。反馈问题时，可在网络信息页导出脱敏诊断，分享前请检查 IP、网络名称和路径。

## 开发与验证

原生界面使用 AppKit，认证与后台逻辑复用 Rust 核心。在对应架构的 Mac 上构建与验证：

```sh
bash scripts/package-macos.sh aarch64-apple-darwin
# Intel Mac 使用 x86_64-apple-darwin
python3 scripts/validate-macos.py 'dist/macos-arm64/校园网自动登录.app'
```

云端覆盖双架构构建、共享核心回归、模拟认证与重连、钥匙串操作、原生界面及资源采样。结果和截图见对应构建附件中的 `macos-validation/VALIDATION.md`。

真实校园网、MacBook 合盖与唤醒、系统权限交互和 macOS 13 实机仍需验证；最低部署版本不代表已在对应实机上测试。测试使用隔离目录、专用钥匙串项目和虚构凭据，不操作用户账号。
