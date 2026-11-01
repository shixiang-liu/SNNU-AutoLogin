//! 开机自启：Linux 端口用 systemd 用户单元（由 deb 或 tar.gz 提供）。
//! WSL 这类没有 user manager 总线的环境里 systemctl 会直接失败，
//! 所以一律先问 systemctl、失败再按单元文件状态离线判断，绝不报错退出。
use std::path::PathBuf;

const UNIT: &str = "snnulogin.service";

/// 用户单元目录：与 linux::storage 一样尊重 XDG_CONFIG_HOME。
fn config_home() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        if base.is_absolute() {
            return base;
        }
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".config")
}

/// enable 后的自启软链：没有总线时用它的存在与否代替 systemctl 的答案。
fn want_link() -> PathBuf {
    config_home()
        .join("systemd/user/default.target.wants")
        .join(UNIT)
}

fn systemctl(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("systemctl")
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// systemctl 的答案与软链状态合并：只有明确 enabled 才算已开。
fn parse_enabled(status: Option<&str>, link_exists: bool) -> bool {
    match status {
        Some(status) => matches!(status, "enabled" | "enabled-runtime"),
        None => link_exists,
    }
}

pub fn enabled() -> bool {
    parse_enabled(
        systemctl(&["--user", "is-enabled", UNIT]).as_deref(),
        want_link().exists(),
    )
}

pub fn enable() -> Result<(), String> {
    systemctl(&["--user", "enable", UNIT])
        .map(|_| ())
        .ok_or_else(|| {
            format!("未能启用 {UNIT}：请在有 systemd 的会话里执行 systemctl --user enable {UNIT}")
        })
}

pub fn disable() -> Result<(), String> {
    systemctl(&["--user", "disable", UNIT])
        .map(|_| ())
        .ok_or_else(|| format!("未能停用 {UNIT}：请执行 systemctl --user disable {UNIT}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 自启状态优先信systemctl没有总线才看软链() {
        assert!(parse_enabled(Some("enabled"), false));
        assert!(parse_enabled(Some("enabled-runtime"), false));
        assert!(!parse_enabled(Some("disabled"), true));
        assert!(!parse_enabled(Some("masked"), true));
        assert!(parse_enabled(None, true));
        assert!(!parse_enabled(None, false));
    }
}
