<div align="center">

<img src="docs/assets/logo.svg" width="72" height="72" alt="SNNU 校园网工具图标">

<h1>SNNU<br>校园网自动登录</h1>

<p>连上学校网络，自动登录、掉线重连。</p>

<p>
  <a href="https://github.com/shixiang-liu/SNNU-AutoLogin/releases/tag/v1.1.1"><img src="https://img.shields.io/badge/Windows-v1.1.1-2563eb?style=flat-square" alt="Windows v1.1.1"></a>
  <a href="https://github.com/shixiang-liu/SNNU-AutoLogin/releases/tag/macos-v1.0.1"><img src="https://img.shields.io/badge/macOS-v1.0.1-475569?style=flat-square" alt="macOS v1.0.1"></a>
  <a href="https://github.com/shixiang-liu/SNNU-AutoLogin/releases/tag/android-v1.0.1"><img src="https://img.shields.io/badge/Android-v1.0.1-16a34a?style=flat-square" alt="Android v1.0.1"></a>
</p>

<p><a href="#下载">下载安装</a> · <a href="#界面预览">界面预览</a> · <a href="#开始使用">使用指南</a> · <a href="https://github.com/shixiang-liu/SNNU-AutoLogin/issues">反馈问题</a></p>

</div>

为陕西师范大学校园网制作的轻量工具，支持「校园网」「移动」「联通」三种上网方式。

- **自动连接**：保存账号后自动认证，掉线、换网或唤醒后检查连接。
- **原生运行**：桌面端驻留托盘或菜单栏，安卓端通过通知栏操作。
- **本地保存**：密码在本机加密保存，没有广告或统计 SDK。

## 下载

| 平台 | 下载安装 |
| :--- | :--- |
| **Windows**<br><sub>Windows 10 / 11 · x64</sub> | [单文件 EXE](https://github.com/shixiang-liu/SNNU-AutoLogin/releases/download/v1.1.1/SNNU-AutoLogin.exe) · [免安装 ZIP](https://github.com/shixiang-liu/SNNU-AutoLogin/releases/download/v1.1.1/SNNU-AutoLogin-v1.1.1-windows-x64.zip) |
| **macOS**<br><sub>macOS 13 及以上</sub> | [M 系列芯片](https://github.com/shixiang-liu/SNNU-AutoLogin/releases/download/macos-v1.0.1/SNNU-AutoLogin-v1.0.1-macos-arm64.zip) · [Intel 芯片](https://github.com/shixiang-liu/SNNU-AutoLogin/releases/download/macos-v1.0.1/SNNU-AutoLogin-v1.0.1-macos-x64.zip) |
| **Android**<br><sub>Android 8.0 及以上</sub> | [安装 APK](https://github.com/shixiang-liu/SNNU-AutoLogin/releases/download/android-v1.0.1/SNNU-AutoLogin-android.apk) |

Windows 可直接运行 EXE，ZIP 需先解压。Mac 首次打开请参照[安装说明](docs/macos.md#第一次使用)。[查看更新记录 →](https://github.com/shixiang-liu/SNNU-AutoLogin/releases)

## 界面预览

<p align="center">
  <a href="docs/screenshots/home.png"><img src="docs/screenshots/home.png" width="278" height="360" align="top" alt="Windows 首页" title="Windows · 查看原图"></a>
  <a href="docs/images/macos/home.png"><img src="docs/images/macos/home.png" width="290" height="360" align="top" alt="macOS 首页，模拟会话" title="macOS · 查看原图"></a>
  <a href="docs/images/android/home.png"><img src="docs/images/android/home.png" width="167" height="360" align="top" alt="Android 首页" title="Android · 查看原图"></a>
</p>

<p align="center"><a href="docs/windows.md#界面预览">Windows 更多界面</a> · <a href="docs/macos.md#界面预览">macOS 更多界面</a> · <a href="android/README.md#界面预览">Android 更多界面</a></p>

## 开始使用

1. 连接学校网络。桌面端支持 Wi-Fi 和有线网络，安卓端使用 Wi-Fi。
2. 填写学号、校园网密码和上网方式，点击「保存并连接」。
3. 保持自动登录开启，关闭界面后继续在后台运行。

Windows 可在设置中开启「自动连接学校 Wi-Fi」，断网后尝试连接已保存的 SNNU。Android 如需锁屏后持续重连，请检查「后台与通知」设置。

使用指南：[Windows](docs/windows.md) · [macOS](docs/macos.md) · [Android](android/README.md)

## 体积与内存

| 客户端 | 程序体积 | 后台内存记录 |
| :--- | ---: | ---: |
| Windows | 1.01 MiB | 13.6–58.8 MiB |
| macOS · M 系列 | 1.21 MiB | 13.4 MiB |
| macOS · Intel | 1.25 MiB | 9.7 MiB |
| Android | 56.2 KiB | 23.9 MiB |

<sub>内存及 Mac 体积为既往版本的实测记录，具体版本见测量文档；本次修复未重测真机内存。各平台测量口径不同，以上并非内存上限，详见<a href="docs/performance.md">测试环境、统计口径与原始采样</a>。</sub>

## 反馈与开发

遇到问题，请在 [Issues](https://github.com/shixiang-liu/SNNU-AutoLogin/issues) 说明系统版本、客户端版本和复现步骤，也可通过[邮件](mailto:shixiang.liu@foxmail.com)联系。诊断报告请先检查并脱敏。

[从源码构建](docs/building.md) · [版本与提交约定](CONTRIBUTING.md) · [MIT 许可证](LICENSE)

<details>
<summary>隐私与账号说明</summary>

账号只用于学校认证，不经过开发者服务器，各客户端需要分别填写密码。学校门户使用 HTTP，本机加密保存不代表网络传输加密。

诊断报告不含密码，账号会脱敏；分享前仍请检查 IP、路径等个人信息，不要上传完整配置。

</details>

这是面向陕西师范大学校园网的社区工具，与学校官方客户端无隶属关系。
