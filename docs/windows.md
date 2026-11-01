# Windows 使用指南

当前版本 **v1.0.1**，支持 **Windows 10 / 11 · x64**。

[下载 Windows v1.0.1](https://github.com/shixiang-liu/SNNU-AutoLogin/releases/tag/v1.0.1) · [返回项目首页](../README.md) · [从源码构建](building.md#windows)

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

## Wi-Fi 自愈

默认开启。只有**同时**满足下面三条时，工具才会把 Wi-Fi 连回学校网络：

| 条件 | 判定 | 不满足时 |
| :--- | :--- | :--- |
| ① 外部互联网不可用 | 访问公开检测地址失败 | 外网还能上 → 门户自己故障，**只等门户恢复，绝不动 Wi-Fi** |
| ② 校园门户不可达 | 11 端口候选探测全部失败 | 门户可达 → 正常登录，不需要自愈 |
| ③ 没有有线在线，且当前 Wi-Fi 不是目标学校网络 | `netsh` 读有线链路与当前 SSID | 有线在线 → **不干预 Wi-Fi**；已经连在目标网络上 → 不做重复连接 |

三条都成立后还有两道闸：先确认目标网络**真的在扫描结果里**（在校外不会每轮都发一次注定失败的连接命令），两次连接动作之间至少间隔 **60 秒**。

只连接系统里**已经保存过**的网络配置，不新建、不删除、不修改任何系统网络配置。

| 配置项 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `wifi_self_heal` | `true` | 关闭后完全不做自愈，只保留登录与重连 |
| `ssid` | `"SNNU"` | 自愈目标网络名称，留空按默认学校网络 |

两项都在 `%APPDATA%\SNNU-AutoLogin\config.json` 里，与 macOS / Linux 端同一套字段名，保存账号时会一并写回。

设置页的「断网时自动连回校园 Wi-Fi」开关直接读写 `wifi_self_heal`：勾选或取消会立刻写回配置文件，后台巡检下一轮就读到新值，不需要重开界面，也不会中断当前连接。开关只改配置，不执行任何网络动作。

只读排障命令（不改变任何网络状态）：

```powershell
snnulogin.exe --wifi-info     # 有线 / Wi-Fi 现状 + 目标网络当前是否可见
snnulogin.exe --doctor        # 状态 + 最近日志
```

## 关闭界面与退出

托盘菜单把「关掉界面」和「退出程序」拆成两项，因为它们做的事完全不同：

| 操作 | 界面 | 后台巡检与自愈 | 什么时候用 |
| :--- | :--- | :--- | :--- |
| 点窗口右上角 × / 托盘「关闭界面」 | 关闭 | **继续运行** | 平时只想要自动登录和 Wi-Fi 自愈，不想留窗口 |
| 托盘「退出并停止后台」 | 关闭 | **一起退出** | 要彻底停掉工具，比如卸载前、或想完全手动控制网络 |
| 开机运行开启 | 登录 Windows 后自动出现在托盘 | 由开机自启直接拉起 | 一次设置，长期免管 |

也就是说，关掉窗口不代表工具停了：托盘图标还在，掉线重连和 Wi-Fi 自愈都照常工作；只有「退出并停止后台」才会让进程彻底消失。开机运行（`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 里的 `--daemon`）走的是同一条后台巡检路径，与界面开关无关。

设置页底部会显示当前后台状态：「后台运行中，关闭界面后仍会继续巡检」或「后台已停止，登录与自愈都不会运行」。

需要退出校园网认证时，使用首页的「断开」。Wi-Fi 自愈关闭时，工具不会替你切换 Wi-Fi。

内存与程序体积见 [性能记录](performance.md)。遇到问题可在网络信息页导出脱敏诊断，分享前请检查个人信息。
