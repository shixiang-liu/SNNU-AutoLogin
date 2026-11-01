# 三平台合并记录（fork-main）

本文件记录 W4 任务：把三条平台分支合并成统一产品主干 `fork-main`，并完成三平台验证矩阵。
全程未 push、未新增远端。

## 0. 分支取入方式（不加远端、不 push）

三条平台分支原本只存在于同机克隆里，主克隆 `D:\U\SNNU-AutoLogin` 只有
`feature/wifi-self-heal @ 7a580f2` 与 `main`：

| 分支 | 提交 | 原本所在克隆 |
| --- | --- | --- |
| feature/macos-self-heal | e4d4b98 | D:\U\SNNU-AutoLogin-macos |
| feature/linux-port | 343ee99 | D:\U\SNNU-AutoLogin-linux |
| feature/android-self-heal | 5773de1 | D:\U\SNNU-AutoLogin-android |

取入方式（只写本地 ref，不建 remote）：

```sh
git fetch D:/U/SNNU-AutoLogin-macos feature/macos-self-heal:refs/heads/feature/macos-self-heal
git fetch D:/U/SNNU-AutoLogin-linux feature/linux-port:refs/heads/feature/linux-port
git fetch D:/U/SNNU-AutoLogin-android feature/android-self-heal:refs/heads/feature/android-self-heal
```

`git remote -v` 全程只有 origin 一条，未 push。

## 1. 合并拓扑

```
*   0db9e46 Merge feature/android-self-heal: Android Wi-Fi self-heal on the portal-unreachable path
|\
| * 5773de1 Add Android Wi-Fi self-heal on the portal-unreachable path
* |   fda949b Merge feature/linux-port: 三平台共用 wifi_heal_core，Linux 改为 WifiAdapter 适配器
|\ \
| * | 343ee99 feat(linux): 新增 Linux 无头端口与 deb/tar.gz 打包
| |/
* |   687add9 Merge feature/macos-self-heal: shared Wi-Fi decision core + macOS adapter
|\ \
| * | e4d4b98 Port the Wi-Fi self-heal to macOS behind a shared decision core
|/ /
* / 7a580f2 Gate the Wi-Fi self-heal connect on target visibility and bound netsh calls
|/
* 708247b Add Windows Wi-Fi self-heal when there is no network at all
```

## 2. 冲突裁决记录

### 2.1 src/agent.rs（冲突）

- 双方意图：macOS 侧把 `HealState` / `heal_on_unreachable` 统一到 `crate::wifi_heal_core`
  （`cfg(any(windows, target_os = "macos"))`）；Linux 侧在 708247b 基线上自带一份
  `linux::wifi::{HealState, heal_on_unreachable}`（`cfg(target_os = "linux")`）。
- 取舍：保留 `wifi_heal_core` 为唯一决策实现，两处挂点合并为
  `cfg(any(windows, target_os = "macos", target_os = "linux"))` 并统一调用共享核心。
- 理由：三条件（外网不通 / 门户不可达 / 不在校园网里）的语义三平台完全一致，只有探测手段不同；
  留两份决策必然漂移。并保留 Linux 侧新增的暂停分支 `sleep(1, ..)` 与 macos/linux 各自
  `pub(crate) fn sleep`（共享核心的等待闭包依赖它）。
- 未使用 ours/theirs 整取。

### 2.2 src/config.rs（冲突）

- 双方意图：Linux 侧给 `wifi_self_heal` / `ssid` 加 `cfg(any(windows, target_os = "linux"))`；
  macOS 侧已把字段改成无条件（三平台的 `Config` 字面量与序列化因此一致）。
- 取舍：取无条件字段，注释改为三平台；删除 Linux 侧重复的
  `#[cfg(target_os = "linux")] fn default_ssid()`——它的值 `"SNNU"` 与
  `wifi_heal_core::DEFAULT_SSID` 相同，保留两个真值来源没有意义。
- 自动合并保留的部分：`app_dir()` / `local_time()` 的 linux 分支、测试里锁文件清理的
  `cfg(any(target_os = "macos", target_os = "linux"))`。

### 2.3 自动合并（无冲突，逐条确认取舍）

- `src/main.rs`：三平台挂载取并集（`mod linux` 声明、`linux::run()` 与既有 macos/windows 入口并存）。
- `Cargo.toml`：`cfg(target_os = "linux")` 的 libc 段与 `cfg(target_os = "macos")` 依赖段并存。
- `src/portal.rs`：`local_naive_now` 的 linux 分支叠加在 macos 分支之后。

### 2.4 超出「取并集」的编码取舍：删除 Linux 自带决策核心

- 删除 `linux::wifi` 的 `Skip/Decision/Outcome/decide/HealState/record/heal/attempt/heal_on_unreachable`
  （重复决策，约 200 行），改为 `Probe` + `impl WifiAdapter for SystemCmd`，形状对齐
  `windows_wifi::NetshProbe` 与 `macos::wifi_heal::Probe`，并新增 nmcli 扫描可见性探测
  （`nmcli -t -f SSID dev wifi list`）与 `parse_scan_visible`。
- Linux 专属能力完整保留：无头 CLI（`--status/--daemon/--setup/--wifi-info/--logoff/--doctor/--probe/--stop`）、
  systemd 单元、flock 单实例锁、SIGTERM 退出、XDG 路径、连接打桩开关 `SNNU_ALLOW_WIFI_CONNECT`。
- 测试：真值表与挂点类测试改为对 `wifi_heal_core::{decide, heal}` 断言（打桩命令入口不变），
  解析类测试原样保留，另补「扫描可见性解析」「看不到目标网络时跳过并占用冷却」两个，18 → 20。
- 有意保留的行为差异（已写进 `src/linux/wifi.rs` 模块头注释）：
  1. 原 `Skip::NoNmcli` 这一档决策消失；nmcli 缺失时探针只给保守答案，连接动作归
     `ConnectReq::Unsupported` → `Outcome::Unsupported{reason: "系统没有 nmcli"}` 并写日志。
     结果同样是一条连接命令都不发，也不会新建或改动 NetworkManager 配置。
  2. nmcli 缺失时条件 1（外网探测）如今会先执行一次（共享核心先判条件 1），
     旧实现是能力检查优先短路；对用户不可见，代价是一次极小的 HTTP 探测。

### 2.5 未触碰

`vendor/` 未改动；`android/` 与 `src/macos/` 只由各自分支的合并带入（见 §3 的漂移检查）。

## 3. 三平台验证矩阵

### V1 Windows

```
cargo build --release --locked --target x86_64-pc-windows-msvc   -> Finished release profile
cargo test --locked --target x86_64-pc-windows-msvc             -> 109 passed; 0 failed; 1 ignored
target/x86_64-pc-windows-msvc/release/snnulogin.exe             -> 1,037,824 bytes
```

### V2 Linux（WSL，主克隆 /mnt/d/U/SNNU-AutoLogin，CARGO_TARGET_DIR=/root/snnu-target-merge）

```
cargo test --locked                    -> 100 passed; 0 failed
cargo test --locked linux::wifi::tests -> 20 passed
cargo test --locked wifi_heal_core::tests -> 10 passed   （共享核心在 Linux 上真跑，合并红利）
cargo build --release --locked         -> ELF 64-bit LSB pie executable, x86-64 (stripped), 1,050,080 bytes
./snnulogin --status                   -> 门户 202.117.144.205:8605 能连上；状态 已在线（425***le）· IP 10.138.108.230；外网 能上；网卡 eth0；有线 已连接；nmcli 缺失；exit=0
./snnulogin --wifi-info / --probe / --doctor / --version -> exit=0
timeout 20 ./snnulogin --daemon         -> exit=124（20s 内一直活着），login.log「后台已启动 v1.0.1」→ 20s 后「后台已退出」，daemon.lock 由 flock 占住
```

测试数对账：88（feature/linux-port 基线）− 18（被删的重复决策测试）+ 20（改写后）
+ 10（共享核心并入）= 100。

### V3 macOS

```
cargo check --locked --target aarch64-apple-darwin -> Finished（2 条既有 dead_code 警告）
cargo check --locked --target x86_64-apple-darwin  -> Finished（同上）
```

两条警告分别来自 `src/portal.rs:351`（`status_quick` 未使用）与 `src/tray_model.rs:14`
（`Event` 未使用），与本次合并无关（两个文件都不在冲突范围内）。

### V4 Android

```
offline-check.sh（REPO 指向合并后的主干）-> 8/8 类全 OK，共 96 个测试
WifiSelfHealTest 11 + ConnectionPolicyTest 6 + PortalProtocolTest 17 + PortalClientTest 14
+ AccountUpdateTest 10 + ConnectionInspectionTest 3 + UiModelTest 29 + SystemSettingsStateTest 6
```

`D:/U/_android-tools/offline-check.sh` 里 `REPO` 写死指向 `SNNU-AutoLogin-android`；
为验证合并后的主干，用同一脚本把 `REPO` 改为 `D:/U/SNNU-AutoLogin`（其余不变，输出目录另开）
跑了一遍，结果同上。

### V5 漂移检查（合并后主干 vs 各分支/基线）

```
git diff 7a580f2 HEAD -- vendor/                                       -> 空
git diff feature/android-self-heal HEAD -- android/                    -> 空
git diff feature/macos-self-heal HEAD -- src/macos/ docs/macos.md      -> 空
git diff feature/macos-self-heal HEAD -- src/windows_wifi.rs           -> 空
git diff feature/linux-port HEAD -- src/linux/ packaging/linux/ scripts/package-linux.sh
                                                                       -> 只有 src/linux/wifi.rs（有意改写）
                                                                          与 src/linux/cli.rs（1 行提示文案）
```

## 4. 遗留风险

1. **nmcli 缺失路径的日志与探测顺序变化**（见 §2.4）：不再是独立的一档「跳过」决策，
   且条件 1 的外网探测会先执行一次。功能上同样零命令、零配置改动，属可控行为差异。
2. **三份适配器的「能力缺失」语义仍不统一**：Windows 无此概念、macOS 用 `CmdError::Missing`、
   Linux 用 `available()` 预检。共享核心只保留 `Unsupported` 一种表达，未做进一步抽象。
3. **单元测试并发写 login.log 会交织**（见 WSL `--daemon` 冒烟后的 log 片段）：
   这是 `logf` 既有行为，与本次合并无关，未改动。
4. **Android 与 Rust 侧是两份独立决策**（Java 无法复用 Rust 核心），语义靠测试对齐，未做跨语言同源化。
5. **Linux 打包（deb / tar.gz）未在合并后重跑**：本次只验证到 `cargo build --release` 产出 ELF；
   重跑需注意 drvfs 权限问题（`CARGO_TARGET_DIR` 指向 WSL 本地盘）。
6. **macOS 只做了 cargo check 双目标**，未在真机跑测试（本机无 macOS 环境）。
