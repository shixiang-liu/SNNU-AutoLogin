# 从源码构建

只想安装使用？请到 [下载页面](../README.md#下载) 获取对应平台版本。本页面向开发者。

## Windows

需要 Rust stable、Visual Studio Build Tools 的「使用 C++ 的桌面开发」组件，以及 Windows SDK。在仓库根目录运行：

```powershell
./scripts/package.ps1
```

脚本会检查格式、运行回归测试并生成发布包：

- `dist/SNNU-AutoLogin.exe`：可直接运行的单文件程序。
- `dist/SNNU-AutoLogin-v<版本>-windows-x64.zip`：程序、说明与许可证。
- `dist/SHA256SUMS.txt`：发布文件校验值。

只编译程序时可以运行：

```powershell
cargo build --release --locked --target x86_64-pc-windows-msvc
```

## macOS

需要 macOS、Xcode Command Line Tools、Rust stable 和 Python 3。在对应架构的 Mac 上运行：

```sh
bash scripts/package-macos.sh aarch64-apple-darwin
# Intel Mac 使用 x86_64-apple-darwin
```

安装包与校验值写入 `dist/`。构建默认使用 ad-hoc 签名，权限说明和验证范围见 [macOS 文档](macos.md)。

### 在 CI 上出包（本机不是 macOS 时）

macOS 产物只能由 macOS runner 构建，交给 [`.github/workflows/macos.yml`](../.github/workflows/macos.yml)：

- 触发：`workflow_dispatch` 手动触发（Actions 页面选 “macOS build and validation” → Run workflow），或 push 改动命中 `src/**`、`vendor/**`、`Cargo.*`、`build.rs`、`.cargo/**`、`scripts/*macos*`、`scripts/licenses/**`、workflow 自身时自动触发；PR 同样会跑。
- 矩阵：`macos-15` → `aarch64-apple-darwin`（arm64），`macos-15-intel` → `x86_64-apple-darwin`（x64）。
- 期望产物：`SNNU-AutoLogin-v<版本>-macos-arm64.zip`、`SNNU-AutoLogin-v<版本>-macos-x64.zip`，以及构建附件里的 `macos-validation/VALIDATION.md`。

## Linux

无头版，产物是 deb 与 tar.gz。需要 Rust stable、`dpkg-deb`、`tar`：

```sh
bash scripts/package-linux.sh
# 产物：dist/linux/snnulogin_<版本>_<架构>.deb
#       dist/linux/snnulogin-<版本>-linux-<架构>.tar.gz
#       dist/linux/SHA256SUMS
```

只编译二进制时可以运行 `cargo build --release --locked`，再用 `SNNU_BIN=<路径> bash scripts/package-linux.sh` 打包已有产物。架构默认取 `dpkg --print-architecture`，可用 `ARCH=` 覆盖。

### 在 WSL 里构建

Windows 上开发时最常见的做法是在 WSL 里构建，有两点必须注意：

1. **`CARGO_TARGET_DIR` 要指到 WSL 本地盘**。仓库通常位于 `/mnt/d/...`（drvfs），那里的文件权限一律被读成 777，`dpkg-deb` 会因为 control 目录权限不是 0755 而失败，`tar` 也会把 777 原样打包进去：

   ```sh
   cd /mnt/d/U/SNNU-AutoLogin
   export CARGO_TARGET_DIR=/root/snnu-target-merge
   bash scripts/package-linux.sh
   ```

2. **依赖下载慢时用国内镜像**（rsproxy），写进 `~/.cargo/config.toml`：

   ```toml
   [source.crates-io]
   replace-with = 'rsproxy-sparse'
   [source.rsproxy-sparse]
   registry = "sparse+https://rsproxy.cn/index/"
   [registries.rsproxy]
   index = "https://rsproxy.cn/crates.io-index"
   [net]
   git-fetch-with-cli = true
   ```

安装与卸载闭环自测：

```sh
cd dist/linux && sha256sum -c SHA256SUMS
sudo dpkg -i snnulogin_<版本>_<架构>.deb && snnulogin --version
sudo dpkg -r snnulogin
```

WSL 默认没有 systemd 用户会话，`systemctl --user` 会连不上总线；验证常驻进程时改用 `timeout 20 ./snnulogin --daemon` 前台冒烟。安装、自启与参数说明见 [Linux 安装说明](../packaging/linux/INSTALL.md)。

## Android

需要完整的 JDK 21、Android SDK Platform 37.0 和 Build Tools 36.0.0，无需安装 Android Studio。

```powershell
./scripts/android-package.ps1
```

默认生成经过裁剪优化的测试包，使用独立包名与测试签名。正式签名、Linux/macOS 构建及设备测试见 [Android 开发与验证](android-development.md)。发布密钥和密码不得写入仓库。

### 在 CI 上出包（本机没有 Android SDK 时）

交给 [`.github/workflows/android.yml`](../.github/workflows/android.yml)：

- 触发：push 到 `main` 且改动命中 `android/**`（`*.md` 除外）、`scripts/android-*`、workflow 自身；PR 同样会跑；也可 `workflow_dispatch` 手动触发，手动触发时可选 `sign_release` 用正式密钥签名。
- 模拟器矩阵：API 26、31、33、34、35、36、37。
- 期望产物：`SNNU-AutoLogin-android.apk`（优化后的测试包为 `SNNU-AutoLogin-android-test.apk`）；正式签名任务只上传产物，不自动发布 Release。

## 代码入口

| 路径 | 职责 |
| :--- | :--- |
| `src/` | 四平台共享的门户协议、调度与状态模型 |
| `src/wifi_heal_core.rs` | Wi-Fi 自愈的三条件决策核心与 60 秒冷却，四平台共用 |
| `src/windows_wifi.rs` / `src/macos/wifi_heal.rs` / `src/linux/wifi.rs` | 各平台的探测与连接适配器（`WifiAdapter` 实现） |
| `src/macos/` | AppKit 界面、钥匙串、菜单栏与 macOS 系统适配 |
| `src/linux/` | 无头 CLI、systemd 单元、flock 单实例锁与 Linux 系统适配 |
| `packaging/linux/` | deb / tar.gz 的单元文件与安装说明 |
| `android/app/src/main/` | Android 界面、服务、网络与账号存储 |
| `android/app/src/test/` | Android 离线回归测试 |
| `android/app/src/androidTest/` | Android 设备检查 |
| `scripts/` | 构建、打包与测量脚本 |
| `.github/workflows/` | 持续集成 |

提交改动时，请说明解决的问题与验证方式，并避免提交账号配置、签名密钥、日志或未脱敏截图。
