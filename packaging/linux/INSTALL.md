# SNNU-AutoLogin Linux 无头版安装说明

无托盘、无图形界面：一个常驻进程按周期探测校园网门户，未认证就自动提交登录，掉线后自动重连。
只访问校园网 HTTP 门户，**不需要 root**，也不改动任何系统网络设置。

## 一、deb 安装（推荐）

```bash
sudo dpkg -i snnulogin_<版本>_<架构>.deb
snnulogin --version
```

安装位置：

| 路径 | 说明 |
| --- | --- |
| `/usr/bin/snnulogin` | 无头主程序 |
| `/usr/lib/systemd/user/snnulogin.service` | systemd 用户单元（`Type=simple`、`ExecStart=/usr/bin/snnulogin --daemon`） |
| `/usr/share/doc/snnulogin/` | 本说明与版权信息 |

卸载：`sudo dpkg -r snnulogin`（配置和日志留在用户目录，不会被删）。

## 二、tar.gz 安装

```bash
tar -xzf snnulogin-<版本>-linux-<架构>.tar.gz
cd snnulogin-<版本>-linux-<架构>
install -Dm755 snnulogin ~/.local/bin/snnulogin      # 或 sudo install -Dm755 snnulogin /usr/local/bin/snnulogin
install -Dm644 systemd/snnulogin.service ~/.config/systemd/user/snnulogin.service
```

tar.gz 里的单元文件按 deb 布局写的是 `/usr/bin/snnulogin`，
如果二进制装在别处（例如 `~/.local/bin`），先用编辑器把 `ExecStart=` 改成实际路径：

```ini
ExecStart=%h/.local/bin/snnulogin --daemon
```

校验完整性：`sha256sum -c SHA256SUMS`。

## 三、保存账号

```bash
snnulogin --setup --account 学号 --password 密码
# 运营商出口（可选）：--operator mobile | --operator unicom；留空即校园网
snnulogin --status
```

不带 `--account` / `--password` 时会在终端里提示输入，密码不回显。

## 四、开机自启

用户单元需要 systemd 用户会话：

```bash
systemctl --user enable --now snnulogin     # 立即启动并开机自启
systemctl --user status snnulogin
journalctl --user -u snnulogin -f
```

没登录桌面会话也想自启，需要给用户开启 linger：

```bash
sudo loginctl enable-linger $USER
```

WSL 默认没有 systemd 用户会话（PID 1 是 init，`systemctl --user` 会报连不上总线），
此时改用前台运行或自己写 nohup/cron 拉起：`snnulogin --daemon &`。

## 五、日常命令

| 命令 | 说明 |
| --- | --- |
| `snnulogin --status` | 只读状态：门户探测、认证状态、外网、后台、自启、网卡、配置路径 |
| `snnulogin --wifi-info` | 只读：有线/Wi-Fi 现状与 nmcli 可用性 |
| `snnulogin --logoff` | 断开认证并暂停自动登录（`--logout` 同义） |
| `snnulogin --doctor` | 状态 + 最近日志 |
| `snnulogin --stop` | 通知后台退出 |
| `snnulogin --help` | 全部参数 |

## 六、Wi-Fi 自愈（可选）

和 Windows 端口同一套三条件门控：**外网不可用 + 门户不可达 + 没有有线在线且当前 Wi-Fi 不是目标网络**
三条同时成立，才会把 Wi-Fi 连回学校网络（只连系统已保存的配置，不扫描、不新建配置）。
有线判定读 `/sys/class/net`，SSID 读 `nmcli -t -f active,ssid dev wifi list`；
**系统没有 nmcli 时整块能力自动跳过**，只记一行日志，不影响登录主流程。

连接动作默认是**打桩**的：只记录一行「本应执行 nmcli dev wifi connect <SSID>」，
不会真的切换无线。确实需要真实切换时，给守护进程加环境变量：

```ini
# ~/.config/systemd/user/snnulogin.service.d/connect.conf
[Service]
Environment=SNNU_ALLOW_WIFI_CONNECT=1
```

## 七、配置、日志与密码

| 路径 | 说明 |
| --- | --- |
| `~/.config/snnulogin/config.json` | 账号、出口、保活与自愈设置（权限 0600） |
| `~/.config/snnulogin/state.json` | 运行状态：控制端口、上次成功时间、上次失败原因等 |
| `~/.config/snnulogin/login.log` | 日志（超过 200 KB 轮转为 `login.log.old`） |

`SNNU_DATA_DIR` 可以把上面整目录换到别处（测试和多实例隔离用）。

密码说明：Linux 上没有 Windows 的 DPAPI，也没有 macOS 的钥匙串，
密码以带标记的 base64 存在 `config.json` 里，**这不是加密**——
目录和文件权限（0700/0600）是本端唯一的防线。能读到该文件的账号等于能读到密码。
Windows 上保存的 DPAPI 密码在 Linux 上解不开，需要在本机重新 `--setup`。
