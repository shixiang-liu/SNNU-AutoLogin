# 体积与内存记录

这些数据用于说明测量条件与实际占用，不是所有设备上的性能保证。Windows 工作集、macOS physical footprint 与 Android PSS 的统计口径不同，不能直接横向排名。

## Windows v1.1.1

2026-11-03 发布构建的 EXE 为 1,054,720 字节（约 1.01 MiB），SHA256：`1b0962c1ed5922e00a4895c4d185f18eb058d6ecf22f940a85b3ee2c6ebed391`。本次是连接恢复与状态保护修复；尚未重新测量 Windows 和 Android 的真机内存、长期待机耗电。其他平台的历史记录仍保留其原始版本与测量条件。

## Windows v1.1.0

2026-11-03 发布构建的 EXE 为 1,034,240 字节（约 0.99 MiB），SHA256：`7e54e41baaa3997e995f1343abe7546161ab2aad26c398d84c8ef22c0c5f24eb`。本次新增 Wi-Fi 自愈，尚未重新测量内存和长期待机耗电；首页 Windows 内存仍引用下面明确标注版本的历史记录。

## Windows v1.0.1

测量日期 2026-10-27，Windows 11 x64（10.0.26200），真实校园网在线。使用 GitHub 发布的 **v1.0.1 EXE**，大小 1,025,536 字节（约 0.98 MiB），SHA256 为 `91954a21fd72c8cf22273e8ed98db174f203ed73e9a973e429fc6c561bba2f0f`。

每组预热 10 秒，每 2 秒采样一次，共 30 次，以下为均值。未清空工作集或人为触发内存回收，测量前后均确认校园网在线。

| 场景 | 进程数 | 总工作集 | 私有工作集 | 可共享工作集 | 私有提交量 |
| :--- | ---: | ---: | ---: | ---: | ---: |
| 冷启动后台，窗口关闭 | 1 | 13.61 MiB | 1.69 MiB | 11.92 MiB | 2.37 MiB |
| 连续运行两小时以上的后台，窗口关闭 | 1 | 58.75 MiB | 5.77 MiB | 52.97 MiB | 38.30 MiB |
| 首页可见，后台同时运行 | 2 | 120.50 MiB | 32.49 MiB | 88.00 MiB | 74.54 MiB |

总工作集包含私有与可共享的驻留页面；私有提交量包含当前不驻留的已提交内存，不能当作实际物理占用。定义见 Microsoft 的 [工作集页面标记](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-psapi_working_set_block)与[进程内存计数器](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex)。各列分别取均值并四舍五入，显示值可能相差 0.01 MiB。

前台行是界面与后台两个进程之和，共享页面可能重复计入，不能当作系统新增的物理内存消耗。脚本按应用主窗口类 `SNNUDesktop` 判断前台，排除输入法状态窗口，并检查采样期间进程集合保持不变。

持续运行的进程加载了搜狗输入法及语音、聊天等扩展模块；随后重启同一份后台 EXE，未退出校园网认证，冷启动组未发现这些模块，工作集恢复到约 14 MiB。模块加载与占用差异相关，但本次没有单独控制输入法加载，不能将全部差异归因于输入法，也不能据此认定或排除内存泄漏。冷启动的低占用不代表长期峰值。

此前 v1.0.0 的记录为总工作集约 14 MiB、私有提交量约 2.6 MiB；本次 v1.0.1 冷启动结果与其接近。旧版缺少逐次原始采样和模块清单，不能据此对比版本回归。

[完整原始采样与环境记录](measurements/windows-v1.0.1-x64.json)。未测试断网等待、整夜运行和待机耗电。

### 复测

保持已连接状态，打开首页时测 `Foreground`；关闭窗口、保留托盘后台时测 `Background`。脚本不负责连接、断开或重启程序，也不会导出账号、IP 和本地程序路径。请分别指定输出文件，避免覆盖记录。

```powershell
./scripts/windows-measure.ps1 -Exe C:/path/to/SNNU-AutoLogin.exe -Scenario Foreground -Output dist/windows/foreground.json
./scripts/windows-measure.ps1 -Exe C:/path/to/SNNU-AutoLogin.exe -Scenario Background -Output dist/windows/background.json
```

## macOS v1.0.0

来源：[双架构构建与验证](https://github.com/shixiang-liu/SNNU-AutoLogin/actions/runs/40517224607)，源码 `83401fc`，测量日期 2026-10-27。程序报告版本与 App 元数据均已核对为 **1.0.0**。

系统为 macOS 15.7.9，使用优化构建和模拟门户；每种场景预热 10 秒，再每 2 秒采样，持续 60 秒。下面是采样均值。

| 架构 | App 体积 | 前台物理占用 | 在线后台物理占用 | 等待网络时物理占用 |
| :--- | ---: | ---: | ---: | ---: |
| Apple Silicon | 1,265,079 字节 | 13.28 MiB | 13.39 MiB | 13.56 MiB |
| Intel | 1,313,239 字节 | 9.75 MiB | 9.70 MiB | 9.76 MiB |

App 体积指 `.app` 内文件总量，不是外层 ZIP 大小。在线后台平均 CPU 分别为单核的 0.127% 与 0.306%。

- [Apple Silicon 测量摘要](measurements/macos-v1.0.0-arm64.json)
- [Intel 测量摘要](measurements/macos-v1.0.0-x64.json)

云端验证不等同于真实校园网和 MacBook 合盖测试，也没有验证 macOS 13 实机。

## Android v1.0.0

正式 APK 为 **56,516 字节（55.2 KiB）**。一加 11 / Android 16 的优化测试构建，每组采样 30 次、间隔 2 秒：

| 场景 | PSS 中位数 | 最大值 |
| :--- | ---: | ---: |
| 前台页面可见 | 96.9 MiB | 97.3 MiB |
| 后台服务在线 | 23.9 MiB | 24.3 MiB |

测试构建与正式包使用独立包名。详细条件与尚待验证项目见 [Android 验证记录](../android/VALIDATION.md)。
