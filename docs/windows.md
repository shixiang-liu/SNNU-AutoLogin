# Windows 使用指南

当前版本 **v1.1.1**，支持 **Windows 10 / 11 · x64**。

[下载 Windows v1.1.1](https://github.com/shixiang-liu/SNNU-AutoLogin/releases/tag/v1.1.1) · [返回项目首页](../README.md) · [从源码构建](building.md#windows)

## 第一次使用

1. 下载 EXE 直接运行；选择 ZIP 时，先解压再运行。
2. 连接学校 Wi-Fi 或有线网络，填写学号、校园网密码和上网方式，点击「保存并连接」。
3. 保持自动登录开启。关闭窗口后，工具继续在托盘运行。

## 界面预览

点击截图可查看原图。

### 连接首页

<p align="center">
  <a href="screenshots/home.png"><img src="screenshots/home.png" width="280" alt="Windows 连接首页"></a>
</p>

### 账号与设置

<p align="center">
  <a href="screenshots/account.png"><img src="screenshots/account.png" width="280" align="top" alt="Windows 账号设置"></a>
  <a href="screenshots/settings.png"><img src="screenshots/settings.png" width="280" align="top" alt="Windows 设置"></a>
</p>

### 功能介绍与网络信息

<p align="center">
  <a href="screenshots/features.png"><img src="screenshots/features.png" width="280" align="top" alt="Windows 功能介绍"></a>
  <a href="screenshots/network.png"><img src="screenshots/network.png" width="280" align="top" alt="Windows 网络信息"></a>
</p>

## 日常操作

关闭窗口会保留后台自动登录；从托盘退出应用会停止后台，保留当前认证连接。需要退出校园网认证时，使用首页的「断开」。默认由 Windows 管理 Wi-Fi 连接。

内存与程序体积见 [性能记录](performance.md)。遇到问题可在网络信息页导出脱敏诊断，分享前请检查个人信息。

## 自动连接学校 Wi-Fi

Windows v1.1.0 新增此功能，默认关闭。macOS 和 Android 保持现有行为。

首次使用先在 Windows 中手动连接并保存 `SNNU`，再保存校园网账号，在设置页开启「自动连接学校 Wi-Fi」并保持自动登录开启。断网后，仅在没有其他 Wi-Fi 或物理有线连接、外网不可用、门户不可达，且附近存在已保存的 `SNNU` 时请求连接。

- 不创建 Wi-Fi 配置，不读取 Wi-Fi 密码，不修改系统自动连接选项、代理或路由。
- 已有其他 Wi-Fi、有线连接、系统正在连接或网络信息不可读时，不干预。
- 请求连接后短周期等待 IP 就绪，随后检查门户。失败或尚未完成的请求间隔至少 60 秒；成功连上 Wi-Fi 后结束本轮冷却，之后重新开关 Wi-Fi 不必等上一轮计时。
- 无线断开、扫描完成及连接结果会唤醒后台；尚无网络地址时先处理 Wi-Fi 连接，避免先等待门户请求超时。
- 用户关闭自动登录、主动断开、修改账号或退出后台后，不再继续本轮认证。已经交给 Windows 的关联请求可能完成，工具不会为取消任务而断开现有 Wi-Fi。
- 仅关闭此开关会停止新的 Wi-Fi 自愈动作；独立的校园网自动登录仍遵循「自动登录」开关。
- 系统服务或权限不可用时保留原有连接，在诊断日志中说明原因，不自动申请权限。

升级前先从托盘退出原程序，再解压并打开新版，避免旧后台仍在处理连接。学号与账号设置会继续保留。

如果在 Windows 的 Wi-Fi 菜单断开 SNNU，工具会尝试恢复；在软件内点击「断开」或关闭自动登录，则会暂停恢复。系统 Wi-Fi 开关关闭时，工具不会擅自打开它。

已完成离线回归、本机 WLAN 检查和用户校园网试用确认。不同无线驱动、锁屏/唤醒、权限拒绝及长期待机仍待进一步验证，不承诺固定秒数完成联网。
