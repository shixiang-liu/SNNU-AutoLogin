//! Linux 无头 CLI：参数语义与 Windows 端口对齐（--status/--daemon/--setup/--wifi-info/--logoff）。
use crate::agent;
use crate::config;
use crate::linux::wifi::Cmd;
use crate::linux::{storage, wifi};
use crate::{autostart, network};
use crate::portal::{self, Status};
use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

/// 门户探测超时：和 Windows 端口的 --status 一致
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let value = |flag: &str| -> Option<String> {
        let at = args.iter().position(|arg| arg == flag)?;
        args.get(at + 1).cloned()
    };

    if has("--help") || has("-h") {
        usage();
        return 0;
    }
    if has("--version") {
        println!("SNNU-AutoLogin v{}", crate::VERSION);
        return 0;
    }
    if has("--daemon") {
        agent::run_daemon();
        return 0;
    }
    if has("--setup") {
        return setup(&has, &value);
    }
    if has("--wifi-info") {
        wifi_info();
        return 0;
    }
    if has("--logoff") || has("--logout") {
        return logoff();
    }
    if has("--stop") {
        println!(
            "{}",
            if agent::stop_daemon() {
                "stopped"
            } else {
                "not running"
            }
        );
        return 0;
    }
    if has("--probe") {
        let (status, port) = portal::status(Duration::from_secs(5));
        println!("{status:?} port={port}");
        return 0;
    }
    if has("--doctor") {
        for line in status_lines() {
            println!("{line}");
        }
        print_log_tail(40);
        return 0;
    }
    // 无参数时按只读状态处理：systemd/脚本里直接跑也不会做任何写操作
    if has("--status") || args.is_empty() {
        for line in status_lines() {
            println!("{line}");
        }
        return 0;
    }
    eprintln!("无法识别的参数：{}", args.join(" "));
    usage();
    2
}

fn logoff() -> i32 {
    // 先暂停自动重连再下线，避免刚断开又被自动登录拉回来
    if agent::logout_now() {
        println!("已断开连接，自动重连已暂停（--setup 或 --wifi-info 后可恢复）");
        0
    } else {
        println!("不在校园网内，或断开失败");
        1
    }
}

fn setup(has: &dyn Fn(&str) -> bool, value: &dyn Fn(&str) -> Option<String>) -> i32 {
    let (mut cfg, _) = config::load_config();
    let account = match asked(value, "--account") {
        Some(account) => account,
        None => match prompt_line("校园网账号：", false) {
            Some(account) => account,
            None => {
                eprintln!("缺少账号：用 --account 指定，或在终端里输入");
                return 2;
            }
        },
    };
    let password = match value("--password").filter(|value| !value.is_empty()) {
        Some(password) => password,
        None => match prompt_line("校园网密码：", true) {
            Some(password) => password,
            None => {
                eprintln!("缺少密码：用 --password 指定，或在终端里输入");
                return 2;
            }
        },
    };

    cfg.account = account;
    if let Some(operator) = value("--operator") {
        cfg.operator = normalize_operator(&operator);
    }
    if let Some(ssid) = value("--ssid") {
        cfg.ssid = ssid.trim().to_string();
    }
    // 首次建立配置时按文档默认值写入（自愈默认开启）：serde 的默认值只在
    // JSON 里缺字段时生效，全新配置走的是 Config::default()，自愈会是关。
    let fresh = !config::cfg_path().exists();
    if has("--no-wifi-self-heal") {
        cfg.wifi_self_heal = false;
    } else if has("--wifi-self-heal") || fresh {
        cfg.wifi_self_heal = true;
    }

    if let Err(error) = storage::save_config(&cfg, &password) {
        eprintln!("保存失败：{error}");
        return 1;
    }
    // 设置完成后撤销“用户主动断开”，后台下一轮巡检就能接管
    // （守护循环每轮重新读取配置，不需要重启服务）
    if let Err(error) = agent::pause_automatic(false) {
        eprintln!("提示：未能立即恢复自动登录（{error}），后台会在下一轮巡检时重试");
    }

    println!(
        "已保存账号 {}（{}）",
        portal::mask(&cfg.account),
        if cfg.operator.is_empty() {
            "校园网".to_string()
        } else {
            format!("{} 出口", cfg.operator)
        }
    );
    println!(
        "Wi-Fi 自愈　{}",
        if cfg.wifi_self_heal {
            format!("开（目标 {}）", wifi::target_ssid(&cfg))
        } else {
            "关".to_string()
        }
    );
    println!("配置　　{}", config::cfg_path().display());
    println!("后台未运行时已自动拉起；systemd 用户服务里用 systemctl --user restart snnulogin 让它重读");
    0
}

fn asked(value: &dyn Fn(&str) -> Option<String>, flag: &str) -> Option<String> {
    value(flag)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 交互输入：密码不回显（只在真实终端上关闭回显，管道输入照常读）
fn prompt_line(label: &str, secret: bool) -> Option<String> {
    let stdin = std::io::stdin();
    let interactive = stdin.is_terminal();
    print!("{label}");
    let _ = std::io::stdout().flush();
    let hidden = secret && interactive && set_echo(false);
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line).is_ok();
    if hidden {
        set_echo(true);
        println!();
    }
    let line = line.trim().to_string();
    if read && !line.is_empty() {
        Some(line)
    } else {
        None
    }
}

/// 关闭/恢复终端回显；拿不到终端属性时返回 false（照常读，只是会回显）
fn set_echo(on: bool) -> bool {
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut term) != 0 {
            return false;
        }
        if on {
            term.c_lflag |= libc::ECHO;
        } else {
            term.c_lflag &= !libc::ECHO;
        }
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term) == 0
    }
}

/// 运营商出口：只认移动/联通，其余（含已停用的电信）都按默认校园网
fn normalize_operator(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "mobile" | "移动" => "mobile".to_string(),
        "unicom" | "联通" => "unicom".to_string(),
        _ => String::new(),
    }
}

fn wifi_info() {
    let cmd = wifi::SystemCmd;
    println!("网卡　　{}", network::current_name());
    println!(
        "有线　　{}",
        if wifi::wired_connected(&cmd) {
            "已连接"
        } else {
            "没连"
        }
    );
    println!(
        "Wi-Fi　　{}",
        wifi::current_wifi_ssid(&cmd).unwrap_or_else(|| "没连".to_string())
    );
    println!(
        "nmcli　　{}",
        if cmd.available() {
            "可用"
        } else {
            "缺失（Wi-Fi 自愈本轮不动作，也不会新建配置）"
        }
    );
    println!("自愈目标　{}", wifi::target_ssid(&config::load_config().0));
}

/// 只读状态：与 Windows --doctor 的信息项对齐，不做任何写操作
fn status_lines() -> Vec<String> {
    let (cfg, password) = config::load_config();
    let state = config::load_state();
    let (status, port) = portal::status(PROBE_TIMEOUT);
    let mut lines = Vec::new();

    if cfg.account.is_empty() {
        lines.push("账号　　还没配置（用 --setup 保存）".to_string());
    } else {
        lines.push(format!(
            "账号　　{}（{}）",
            portal::mask(&cfg.account),
            if password.is_empty() {
                "没有保存密码"
            } else {
                "密码已保存"
            }
        ));
    }
    if port > 0 {
        lines.push(format!("门户　　{}:{} 能连上", portal::host(), port));
    } else {
        lines.push(format!("门户　　{} 连不上", portal::host()));
    }
    let online = match &status {
        Status::Online(session) => {
            let extra = if session.ip.is_empty() {
                String::new()
            } else {
                format!(" · IP {}", session.ip)
            };
            format!("已在线（{}）{}", portal::mask(&session.account), extra)
        }
        Status::Offline => "连上校园网了，但还没认证".to_string(),
        Status::Unreachable => "不在校园网内".to_string(),
    };
    lines.push(format!("状态　　{online}"));
    lines.push(format!(
        "外网　　{}",
        if portal::internet_ok() {
            "能上"
        } else {
            "上不了"
        }
    ));
    lines.push(format!(
        "后台　　{}",
        if agent::running() {
            format!("运行中（控制端口 {}）", agent::ctl_port())
        } else {
            "没在跑".to_string()
        }
    ));
    lines.push(format!(
        "自启　　{}",
        if autostart::enabled() {
            "已开"
        } else {
            "已关（systemctl --user enable snnulogin）"
        }
    ));
    let cmd = wifi::SystemCmd;
    lines.push(format!("网卡　　{}", network::current_name()));
    lines.push(format!(
        "有线　　{}",
        if wifi::wired_connected(&cmd) {
            "已连接"
        } else {
            "没连"
        }
    ));
    lines.push(format!(
        "Wi-Fi　　{}",
        wifi::current_wifi_ssid(&cmd).unwrap_or_else(|| {
            if cmd.available() {
                "没连".to_string()
            } else {
                "没有 nmcli".to_string()
            }
        })
    ));
    lines.push(format!(
        "自愈　　{}",
        if cfg.wifi_self_heal {
            format!("开（目标 {}）", wifi::target_ssid(&cfg))
        } else {
            "关".to_string()
        }
    ));
    if !state.last_error.is_empty() {
        lines.push(format!(
            "上次失败　{}",
            portal::scrub(&state.last_error, &cfg.account)
        ));
    }
    lines.push(format!("配置　　{}", config::cfg_path().display()));
    lines.push(format!("日志　　{}", config::log_path().display()));
    lines
}

fn print_log_tail(count: usize) {
    let Ok(text) = std::fs::read_to_string(config::log_path()) else {
        println!("最近日志（没有日志文件）");
        return;
    };
    let (cfg, _) = config::load_config();
    let tail: Vec<&str> = text.lines().rev().take(count).collect();
    println!("最近日志（最后 {} 行）", count);
    for line in tail.iter().rev() {
        println!("{}", portal::scrub(line, &cfg.account));
    }
}

fn usage() {
    println!(
        "SNNU-AutoLogin v{}（Linux 无头版）

用法：
  snnulogin --status          只读状态：门户探测、后台、自启、网卡、配置路径
  snnulogin --daemon          常驻后台（systemd 用户单元 Type=simple 的 ExecStart）
  snnulogin --setup [选项]    保存校园网账号
  snnulogin --wifi-info       只读：有线/无线现状与 nmcli 可用性
  snnulogin --logoff          断开认证并暂停自动登录（--logout 同义）
  snnulogin --doctor          状态 + 最近日志
  snnulogin --probe           只打印门户探测结果
  snnulogin --stop            通知后台退出
  snnulogin --version | --help

--setup 选项：
  --account 账号   --password 密码   --operator mobile|unicom（留空即校园网）
  --ssid 目标Wi-Fi（留空即 SNNU）    --no-wifi-self-heal | --wifi-self-heal
  不带 --account/--password 时在终端里提示输入，密码不回显。

配置目录　~/.config/snnulogin（可用 SNNU_DATA_DIR 覆盖；密码是 0600 权限下的 base64，不是加密）
Wi-Fi 自愈　连接动作默认打桩，只有 SNNU_ALLOW_WIFI_CONNECT=1 才会真正切换无线",
        crate::VERSION
    );
}
