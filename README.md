<div align="center">

<img src="docs/assets/logo.svg" width="72" height="72" alt="SNNU 校园网工具图标">

<h1>SNNU<br>校园网自动登录</h1>

<p>连上学校网络，自动登录、掉线重连；没连在学校网上时，自己把 Wi-Fi 连回去。</p>

<p><a href="#下载">下载安装</a> · <a href="#wi-fi-自愈">Wi-Fi 自愈</a> · <a href="#平台说明">平台说明</a> · <a href="#从源码构建">从源码构建</a> · <a href="#与上游的差异">与上游的差异</a></p>

</div>

> **本项目是 fork 增强版。** 基于 [shixiang-liu/SNNU-AutoLogin](https://github.com/shixiang-liu/SNNU-AutoLogin)（MIT）二次开发：上游提供 Windows / macOS / Android 三端的门户协议、原生界面与后台重连，本 fork 在此之上新增**四平台 Wi-Fi 自愈**（Windows / Android / macOS / Linux 共用一套三条件决策核心）与**完整的 Linux 无头版**（deb / tar.gz / systemd 用户服务）。上游代码版权归原作者，本 fork 的增量同样以 MIT 发布。

## 特性

| 能力 | 说明 |
| :--- | :--- |
| **自动登录** | 保存学号、校园网密码与上网方式（校园网 / 移动 / 联通）后自动认证；关闭窗口仍在托盘、菜单栏、通知栏或后台进程里运行。 |
| **掉线重连** | 周期巡检门户状态，掉线、换网、休眠唤醒后自动恢复；失败按退避重试，欠费、验证码一类错误不会把账号刷到锁定。 |
| **Wi-Fi 自愈** | 本身没网、门户也不可达、又不在校园网里，三条同时成立才把 Wi-Fi 连回学校网络。**外网还能上时门户故障绝不动 Wi-Fi**，**有线在线也不干预**。详见 [Wi-Fi 自愈](#wi-fi-自愈)。 |
| **四平台覆盖** | Windows 10/11（托盘）、macOS 13+（菜单栏）、Android 8.0+（前台服务 + 通知）、Linux（无头 CLI + systemd 用户服务）。 |
| **11 端口 failover** | 校园门户在 8600–8610 共 11 个端口上做候选探测，先试上次成功的地址，失败后自动换端口并记住结果。 |
| **凭据只存本机** | 密码在本机保存（Windows DPAPI / macOS 钥匙串 / Android Keystore + AES-GCM / Linux 0600 文件），不经过任何第三方服务器，也没有广告或统计 SDK。 |

## 下载

> 下表为 v1.1.0 的发布资产名，链接在打包与 CI 完成后指向对应 Release。同一版本号在四端保持一致。

| 平台 | 发布资产 |
| :--- | :--- |
| **Windows**<br><sub>Windows 10 / 11 · x64</sub> | `SNNU-AutoLogin-v1.1.0-windows-x64.exe`（单文件）<br>`SNNU-AutoLogin-v1.1.0-windows-x64.zip`（免安装：EXE + README + LICENSE + THIRD-PARTY-NOTICES） |
| **macOS**<br><sub>macOS 13 及以上</sub> | `SNNU-AutoLogin-v1.1.0-macos-arm64.zip`（Apple Silicon）<br>`SNNU-AutoLogin-v1.1.0-macos-x64.zip`（Intel） |
| **Android**<br><sub>Android 8.0 及以上</sub> | `SNNU-AutoLogin-android.apk` |
| **Linux**<br><sub>无头版 · amd64</sub> | `snnulogin_1.1.0_amd64.deb`（Debian/Ubuntu，含 systemd 用户单元）<br>`snnulogin-1.1.0-linux-amd64.tar.gz`（解压即用） |

各平台产物均随附 `SHA256SUMS` / `SHA256SUMS.txt`，下载后建议先校验：

```bash
sha256sum -c SHA256SUMS          # Linux / macOS
certutil -hashfile <文件名> SHA256   # Windows
```

Windows 可直接运行 EXE，ZIP 需先解压。macOS 首次打开请参照[安装说明](docs/macos.md#第一次使用)。Linux 安装三步见下，完整说明见 [packaging/linux/INSTALL.md](packaging/linux/INSTALL.md)。

## 平台说明

### Windows

托盘常驻。首页填写账号并连接后，关闭窗口继续在后台自动登录；从托盘退出会停止后台但保留当前认证连接。

- 首次运行填写学号、密码、上网方式，点「保存并连接」。
- 网络信息页可导出脱敏诊断；Wi-Fi 自愈状态与目标网络也会显示在那里。
- 命令行只读辅助（用于排障，不改变任何网络状态）：

```powershell
snnulogin.exe --wifi-info     # 只读：有线/Wi-Fi 现状、自愈目标网络、网卡
snnulogin.exe --doctor        # 状态 + 最近日志
```

Windows 侧自愈用 `netsh wlan` 读当前网络、扫描可见性并连接系统里已保存的配置，不新建、不修改任何系统网络配置。详见 [Windows 使用指南](docs/windows.md#wi-fi-自愈)。

### macOS

菜单栏常驻，功能与 Windows 对齐（连接 / 断开 / 自动登录开关 / 网络信息）。

- 未签名未公证：首次打开需在「系统设置 → 隐私与安全性」中选「仍要打开」。
- 自愈的有线判定读 `networksetup -listallhardwareports` + `ifconfig`；当前网络优先读 `airport -I`。
- **`airport` 自 macOS 14.4 起已被 Apple 移除**：读不到时回退 `networksetup -getairportnetwork`，扫描不可用时不再扫描，改用两次尝试之间至少 60 秒的冷却限频。
- 连接使用 `networksetup -connectairport`（开放网络免密）；**部分 macOS 版本没有这个子命令，此时只记日志、不做连接动作，返回 `Unsupported`，绝不会退回会新建系统网络配置的 `-setairportnetwork`。**

详见 [macOS 使用指南](docs/macos.md#wi-fi-自愈)。

### Linux（无头版）

没有托盘也没有图形界面：一个 `snnulogin --daemon` 常驻进程按周期探测门户、自动认证、掉线重连。只访问校园网 HTTP 门户，**不需要 root**，也不改动系统网络设置。

安装（三步，deb 与 tar.gz 任选其一）：

```bash
# ① deb
sudo dpkg -i snnulogin_1.1.0_amd64.deb
snnulogin --setup --account 学号 --password 密码
systemctl --user enable --now snnulogin

# ① tar.gz（二进制装到 ~/.local/bin，记得改单元里的 ExecStart）
tar -xzf snnulogin-1.1.0-linux-amd64.tar.gz && cd snnulogin-1.1.0-linux-amd64
install -Dm755 snnulogin ~/.local/bin/snnulogin
install -Dm644 systemd/snnulogin.service ~/.config/systemd/user/snnulogin.service
```

全部参数：

| 参数 | 说明 |
| :--- | :--- |
| `--status` | 只读状态：门户探测、认证状态、外网、后台、自启、网卡、配置路径（不带参数时的默认动作） |
| `--daemon` | 常驻后台，systemd 用户单元的 `ExecStart` |
| `--setup` | 保存账号；可选 `--account` `--password` `--operator mobile\|unicom` `--ssid` `--wifi-self-heal` `--no-wifi-self-heal`，不带账号密码时终端提示输入、密码不回显 |
| `--wifi-info` | 只读：有线 / 无线现状、自愈目标与 nmcli 可用性 |
| `--logoff` | 断开认证并暂停自动登录（`--logout` 同义） |
| `--doctor` | 状态 + 最近日志 |
| `--probe` | 只打印门户探测结果 |
| `--stop` | 通知后台退出 |
| `--version` / `--help` | 版本与全部参数 |

- **systemd 用户服务**：`systemctl --user enable --now snnulogin`、`journalctl --user -u snnulogin -f`；想在没有桌面会话时也自启，需 `sudo loginctl enable-linger $USER`。
- **WSL 限制**：WSL 默认没有 systemd 用户会话（`systemctl --user` 连不上总线），此时改用前台运行 `snnulogin --daemon &`；WSL 也没有 NetworkManager，`nmcli` 缺失时 Wi-Fi 自愈整块自动跳过（只记一行日志），登录主流程不受影响。
- 配置、状态与日志在 `~/.config/snnulogin/`（`SNNU_DATA_DIR` 可整体改到别处）。Linux 上没有 DPAPI 和钥匙串，密码是 0600 权限下的 base64——**不是加密**，目录权限是本端唯一防线。

详见 [Linux 安装说明](packaging/linux/INSTALL.md)。

### Android

单个前台服务 + 常驻通知，支持自动登录与掉线重连（与上游一致；手机连 Wi-Fi 本身由系统自动化接管，本 fork 不做移动端 Wi-Fi 自愈）。

- 只处理校园网认证，**不代替系统连接 Wi-Fi**，不使用 VPN、Root、无障碍服务、精确闹钟或常驻唤醒锁。
- 不申请定位权限，不申请 `CHANGE_WIFI_STATE`；界面上的网络名称固定显示 `SNNU`。
- 关闭「自动登录」即停掉前台服务与通知，当前认证连接保留；锁屏后能否持续重连受厂商省电与自启动设置限制。

详见 [Android 使用指南](android/README.md)。

### iOS

系统不开放 Wi-Fi 控制与后台常驻能力，本仓库不提供 iOS 原生客户端。推荐用系统自带的「快捷指令」实现等效体验：
**加入 SNNU 网络时自动提交门户登录**，无需越狱、无需签名、无需电脑。

三条快捷指令：**登录 / 状态 / 断开**，均可添加到主屏幕，点图标即执行。

详见 [iOS 快捷指令配置指南](ios/shortcuts-guide.md)；自签 ipa 路线的可行性、成本与功能边界见 [iOS ipa 路线评估](ios/ipa-roadmap.md)。

## Wi-Fi 自愈

只在**同时**满足下面三条时动手，缺一不可：

| 条件 | 判定 | 不满足时 |
| :--- | :--- | :--- |
| ① 外部互联网不可用 | 访问公开检测地址失败 | 外网还能上 → 这是门户自己的问题，**只等门户恢复，绝不动 Wi-Fi** |
| ② 校园门户不可达 | 11 端口候选探测全部失败 | 门户可达 → 不需要自愈，正常登录 |
| ③ 没有有线在线，且当前 Wi-Fi 不是目标学校网络 | 有线链路状态 + 当前 SSID | 有线在线 → **不干预 Wi-Fi**；已经连在目标网络上 → 重复连接没有意义 |

三条都成立后还有两道闸：

- **可见性门控**：先确认目标网络真的在扫描结果里，在校外不会每轮都发一次注定失败的连接命令。
- **60 秒冷却**：两次连接动作之间至少间隔 60 秒，避免每轮巡检都去连一次。

只连接**系统里已经保存过**的网络配置：不新建配置、不删除配置、不改任何系统网络设置。

配置开关（`config.json`，四平台字段名一致，Windows 在 `%APPDATA%\SNNU-AutoLogin\`、macOS 在 `~/Library/Application Support/SNNU-AutoLogin/`、Linux 在 `~/.config/snnulogin/`）：

| 字段 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `wifi_self_heal` | `true` | 关闭后完全不做自愈，只保留登录与重连 |
| `ssid` | `"SNNU"` | 自愈目标网络名称，留空按默认学校网络 |

各平台取数与限制：Windows 用 `netsh wlan`；macOS 用 `networksetup` / `ifconfig` / `airport`（`airport` 在 14.4 起被移除，见上）；Linux 有线判 `/sys/class/net`、SSID 读 `nmcli`，系统没有 nmcli 时整块跳过且连接动作默认打桩（`SNNU_ALLOW_WIFI_CONNECT=1` 才真正切换）；Android 走 `WifiManager` 建议或旧接口，未授权定位时读不到 SSID。

## 体积与内存

| 客户端 | 程序体积 | 后台内存记录 |
| :--- | ---: | ---: |
| Windows | 0.98 MiB | 13.6–58.8 MiB |
| macOS · M 系列 | 1.21 MiB | 13.4 MiB |
| macOS · Intel | 1.25 MiB | 9.7 MiB |
| Android | 55.2 KiB | 23.9 MiB |
| Linux · amd64 | 1.00 MiB | 常驻进程，无图形界面 |

<sub>Windows 两项为本机冷启动与加载输入法扩展后的持续运行记录。各平台测量口径不同，以上并非内存上限，详见<a href="docs/performance.md">测试环境、统计口径与原始采样</a>。</sub>

## 隐私与账号说明

<details>
<summary>点开查看</summary>

- 账号和密码只用于学校门户认证，**不经过开发者服务器**，也没有广告或统计 SDK；各客户端需要分别填写密码，互不迁移。
- 密码只保存在本机：Windows 用 DPAPI，macOS 用系统钥匙串，Android 用 Keystore 的 AES-256-GCM，Linux 用 0600 权限文件（**base64，不是加密**）。
- 学校门户使用 **HTTP**：本机存储加密不等于网络传输加密，认证报文在校园网内是明文。
- 诊断报告会脱敏账号与密码，但仍可能包含机型、IP、路径与门户地址；分享前请自行检查，不要上传完整配置或日志。
- Wi-Fi 自愈只连接系统里已经保存的网络，不扫描上报、不新建配置、不改系统网络设置。

</details>

## 与上游的差异

| 项 | 上游 shixiang-liu/SNNU-AutoLogin | 本 fork |
| :--- | :--- | :--- |
| Wi-Fi 自愈 | 无 | **有**：Windows / macOS / Linux 桌面三端共用一套三条件决策核心（`src/wifi_heal_core.rs`），各平台只提供探测与连接适配器。移动端（Android / iOS）不做——手机连 Wi-Fi 由系统自动化接管，无自愈必要 |
| Linux | 无 | **有**：无头 CLI、systemd 用户单元、deb / tar.gz 打包 |
| 平台数 | 3 | **4**（新增 Linux） |
| 电信代拨 | 无 | **无**——上游与学校官方门户都没有这项能力，历史遗留需求不引入，避免伪造不存在的行为 |
| 其余功能 | 门户协议、原生界面、后台重连、账号加密、诊断导出 | 与上游一致，未改动 `vendor/` 与既有业务逻辑 |

致谢：感谢 [@shixiang-liu](https://github.com/shixiang-liu) 开源 [SNNU-AutoLogin](https://github.com/shixiang-liu/SNNU-AutoLogin)，本项目的门户协议实现、界面与打包流程均建立在它的基础上。上游以 MIT 许可发布，本 fork 保留同一许可证与原始版权声明（见 [LICENSE](LICENSE)）。

## 从源码构建

需要 Rust stable；三端 CI 与本地打包脚本的差异见 [构建说明](docs/building.md)。

| 平台 | 命令 | 产物 |
| :--- | :--- | :--- |
| Windows | `./scripts/package.ps1` | `dist/SNNU-AutoLogin.exe`、`dist/SNNU-AutoLogin-v<版本>-windows-x64.zip`、`dist/SHA256SUMS.txt` |
| Linux | `bash scripts/package-linux.sh` | `dist/linux/snnulogin_<版本>_amd64.deb`、`dist/linux/snnulogin-<版本>-linux-amd64.tar.gz`、`dist/linux/SHA256SUMS` |
| macOS | `bash scripts/package-macos.sh aarch64-apple-darwin` | `dist/` 下的 `.zip` 与校验值（需在对应架构的 Mac 上运行） |
| Android | `./scripts/android-package.ps1` | `dist/android/` 下的 APK |

Linux 构建在 WSL 里跑时请把 `CARGO_TARGET_DIR` 指到 WSL 本地盘（`/mnt/d` 是 drvfs，文件权限一律被读成 777，`dpkg-deb` 与 `tar` 都会因此失败），细节见[构建说明的 Linux 段](docs/building.md#linux)。本机三平台打包的实测记录、产物哈希与哪些环节必须交给 CI，见[打包与发版记录](docs/release-packaging.md)。

## 反馈与开发

遇到问题请在 [Issues](https://github.com/shixiang-liu/SNNU-AutoLogin/issues) 说明系统版本、客户端版本和复现步骤；属于本 fork 新增能力（Wi-Fi 自愈、Linux）的问题，也请注明平台与 `--doctor` / 诊断导出结果（先脱敏）。

[从源码构建](docs/building.md) · [Windows](docs/windows.md) · [macOS](docs/macos.md) · [Android](android/README.md) · [Linux](packaging/linux/INSTALL.md) · [MIT 许可证](LICENSE)

这是面向陕西师范大学校园网的社区工具，与学校官方客户端无隶属关系。
