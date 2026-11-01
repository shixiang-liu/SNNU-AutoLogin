# 打包与发版记录（W5 · release-prep）

本页记录 2026-11-01 在 `release-prep`（基线 `fork-main @ dd1050e`）上做的一次三平台打包实测：
做了什么、产物是什么、哪些环节本机做不了必须交给 CI、以及已知风险。**全程未 push、未新增远端、未做任何真实 Wi-Fi 切换。**

## 一、版本号现状

`Cargo.toml` 仍是 `1.0.1`，因此**本次真实产物是 v1.0.1**；[README](../README.md#下载) 的下载表按发版计划写的是 v1.1.0 的**资产名占位**（链接待 CI 出包后再指向 Release）。要让产物名真正变成 v1.1.0，需要把 `Cargo.toml` / `Cargo.lock` 的版本与 `android/app/build.gradle` 的 `versionName` / `versionCode` 一并提升——那属于代码与构建配置改动，不在 W5 的改动范围内，留给发版任务。

## 二、Windows

### 上游脚本的实际执行情况（走退路）

`scripts/package.ps1` 在本机**跑不通**，两个独立原因，均与本 fork 的改动无关：

1. **`cargo metadata` 的 UTF-8 输出被 Windows PowerShell 5.1 按 GBK 解码**，`ConvertFrom-Json` 在第一个非 ASCII 字符处报
   `ConvertFrom-Json : 无效的 JSON 基元`。Cargo.toml 的 `description` 字段含中文，因此必然触发。
   绕过方式：把 `[Console]::OutputEncoding` 设为 UTF-8 后再调用脚本（不是脚本缺陷，是宿主编码问题）。
2. 绕过编码后脚本继续执行，在 `scripts/package.ps1:20` 主动中止：
   `发布脚本需要 Windows x64 MSVC 工具链`。本机 rustup 默认工具链是 `stable-x86_64-pc-windows-gnu`
   （`rustc -vV` → `host: x86_64-pc-windows-gnu`），脚本要求 host 必须是 `x86_64-pc-windows-msvc`。
   交叉编译到 `x86_64-pc-windows-msvc` 本身可用（`cargo build --release --locked --target x86_64-pc-windows-msvc` 成功），
   但脚本的 host 断言会先拦住。

此外，即使换到 MSVC host 工具链，脚本的 `cargo fmt --all -- --check` 也会失败：`src/linux/` 下 5 个文件
（`autostart.rs`、`cli.rs`、`network.rs`、`process_lock.rs`、`wifi.rs`）存在 7 处 rustfmt 差异，
是 `fork-main` 合并时带进来的既有状态，本次未改动（见第五节 `git diff` 证据）。

### 实际采用的退路

```powershell
cargo build --release --locked --target x86_64-pc-windows-msvc
# -> target/x86_64-pc-windows-msvc/release/snnulogin.exe（1,037,824 字节）
# 手工组 zip：EXE + README.md + LICENSE，写 SHA256SUMS.txt
```

产物（`dist/`，已被 `.gitignore` 忽略，不入库）：

| 文件 | 字节 | SHA-256 |
| :--- | ---: | :--- |
| `SNNU-AutoLogin-v1.0.1-windows-x64.zip` | 587,085 | `c2286e31b1032e9397f70204370ba1c9db7d28ca1bebe0e9ade1a496df686cd1` |
| `SNNU-AutoLogin.exe` | 1,037,824 | `d6a20617e3d47d5ede7c76ef196ea8a55f9f69dae4282f64b1a4dc7c713caae5` |
| `SHA256SUMS.txt` | 189 | — |

zip 内含 `SNNU-AutoLogin.exe`、`README.md`、`LICENSE`（**未含 `THIRD-PARTY-NOTICES.txt`**，那一步由 `scripts/third-party.ps1` 生成，同样受上面的 PowerShell 编码问题影响，退路里跳过了）。

二进制自检：

```text
> snnulogin.exe --version
SNNU-AutoLogin v1.0.1
> snnulogin.exe --wifi-info        # 只读
有线　　没连
Wi-Fi　　SNNU
SNNU　　可见
```

回归测试：`cargo test --release --locked --target x86_64-pc-windows-msvc` → **109 passed; 0 failed; 1 ignored**。

## 三、Linux（WSL Ubuntu，合并后主干重跑）

```sh
cd /mnt/d/U/SNNU-AutoLogin
export CARGO_TARGET_DIR=/root/snnu-target-merge   # WSL 本地盘；/mnt/d 是 drvfs，权限一律 777
bash scripts/package-linux.sh
```

脚本本身**无需任何修改**即可跑通（`CARGO_TARGET_DIR` 已指向本地盘，`mktemp -d` 暂存目录也落在本地盘，
因此 `dpkg-deb` 的 0755 权限要求与 `tar` 的权限打包都正常）。产物在 `dist/linux/`：

| 文件 | 字节 | SHA-256 |
| :--- | ---: | :--- |
| `snnulogin_1.0.1_amd64.deb` | 507,896 | `6558b2d4170a39701ba195dcf529a00f9d92b962e7f95c3d22ecebe68bf771fd` |
| `snnulogin-1.0.1-linux-amd64.tar.gz` | 563,320 | `20b8d0f750198bc47d29fed5722833113fa05336f14b0b2f6b5d0b1467404d18` |
| `SHA256SUMS` | 197 | — |

校验与安装闭环：

```text
$ sha256sum -c SHA256SUMS
./snnulogin-1.0.1-linux-amd64.tar.gz: OK
./snnulogin_1.0.1_amd64.deb: OK

$ sudo dpkg -i snnulogin_1.0.1_amd64.deb
Unpacking snnulogin (1.0.1) ...
Setting up snnulogin (1.0.1) ...

$ snnulogin --version
SNNU-AutoLogin v1.0.1
$ ls -l /usr/lib/systemd/user/snnulogin.service
-rw-r--r-- 1 root root 439 ... /usr/lib/systemd/user/snnulogin.service

$ sudo dpkg -r snnulogin
Removing snnulogin (1.0.1) ...
$ dpkg -l snnulogin
dpkg-query: no packages found matching snnulogin
```

tar.gz 内容：`snnulogin`、`README.md`、`INSTALL.md`、`systemd/snnulogin.service`。
deb 的 control 字段：`Package: snnulogin`、`Version: 1.0.1`、`Architecture: amd64`、`Recommends: network-manager`。

## 四、macOS 与 Android（本机不可构建，交 CI）

### macOS

本机是 Windows，无法构建 `.app`。触发 [`.github/workflows/macos.yml`](../.github/workflows/macos.yml)：

- **手动**：Actions → “macOS build and validation” → Run workflow（workflow 声明了 `workflow_dispatch`）。
- **自动**：push 命中 `src/**`、`vendor/**`、`Cargo.*`、`build.rs`、`.cargo/**`、`scripts/*macos*`、`scripts/licenses/**`、workflow 自身；PR 同样触发。
- 矩阵：`macos-15` → `aarch64-apple-darwin`；`macos-15-intel` → `x86_64-apple-darwin`。
- 期望产物：`SNNU-AutoLogin-v<版本>-macos-arm64.zip`、`SNNU-AutoLogin-v<版本>-macos-x64.zip`，附件含 `macos-validation/VALIDATION.md`。

本机只做得到静态检查：`cargo check --locked --target aarch64-apple-darwin` / `--target x86_64-apple-darwin`（W4 已记录，本次未重跑）。

### Android

本机没有 Android SDK，无法执行 `gradlew`。触发 [`.github/workflows/android.yml`](../.github/workflows/android.yml)：

- **自动**：push 到 `main` 且命中 `android/**`（`*.md` 除外）、`scripts/android-*`、workflow 自身；PR 同样触发。
- **手动**：`workflow_dispatch`，可选 `sign_release` 用正式密钥签名。
- 模拟器矩阵：API 26、31、33、34、35、36、37。
- 期望产物：`SNNU-AutoLogin-android.apk`（优化测试包为 `SNNU-AutoLogin-android-test.apk`）；正式签名任务只上传，不自动发布 Release。

## 五、改动范围

本次提交只动文档：`README.md`、`docs/building.md`、`docs/windows.md`、`docs/android-development.md`、新增本页。
`src/`、`vendor/`、`android/`、`scripts/`、`packaging/`、`Cargo.toml`、`Cargo.lock` 均未改动；`NOTES.md` 原样保留。

```sh
git diff --stat fork-main -- src/ vendor/ android/ scripts/ packaging/ Cargo.toml Cargo.lock
# 空
```

## 六、遗留风险

1. **版本号未提升**：产物仍是 v1.0.1，README 下载表按 v1.1.0 占位。发版前必须统一 `Cargo.toml` / `Cargo.lock` / Android `versionName`+`versionCode`，否则资产名对不上。
2. **`scripts/package.ps1` 在本机不可用**：需要 MSVC host 工具链 + UTF-8 控制台。已如实走退路，产物未含 `THIRD-PARTY-NOTICES.txt`。建议在 CI 的 Windows runner（MSVC host）上跑原脚本出正式包。
3. **`cargo fmt --all -- --check` 在 `src/linux/` 有 7 处既有差异**：`package.ps1` 的第一道关卡会失败。是否格式化 `src/linux/` 需要单独决策（属 src 改动，本次红线禁止）。
4. **macOS 与 Android 完全未经本机验证**：只写了 CI 触发方式与期望产物名，产物正确性以 CI 为准。
5. **macOS 的 `airport` 弃用路径**（14.4 起移除）与 **Android 未授权定位时读不到 SSID** 两条，仍未在真机确认，已在 README 与 `docs/android-development.md` 中如实写明。
6. **Linux 连接动作默认打桩**：`SNNU_ALLOW_WIFI_CONNECT=1` 才会真的调 `nmcli` 切换无线；本次未做任何真实切换。
