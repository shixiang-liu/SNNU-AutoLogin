//! Linux 配置与密码存储：没有 DPAPI 也没有钥匙串。
//! 密码用带标记的 base64 存放，文件权限收到 0600——这不是加密，
//! 能读到文件的人就能读出密码，权限是本端唯一的防线（安装说明里写明）。
use crate::config::Config;
use base64::Engine;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// 本端密码标记：用它把“本机保存的密码”和 Windows DPAPI 令牌区分开。
const SECRET_PREFIX: &str = "plain-v1:";
/// 配置目录名：跟 XDG 习惯一致的小写名。
const DIR_NAME: &str = "snnulogin";

pub fn app_dir() -> PathBuf {
    // 显式覆盖：测试和排查时隔离真实配置。
    if let Some(dir) = std::env::var_os("SNNU_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    config_dir(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// XDG 配置目录：XDG_CONFIG_HOME 必须是绝对路径才作数，否则回落到 ~/.config。
fn config_dir(xdg: Option<&OsStr>, home: Option<&OsStr>) -> PathBuf {
    if let Some(base) = xdg.map(PathBuf::from) {
        if base.is_absolute() {
            return base.join(DIR_NAME);
        }
    }
    PathBuf::from(home.unwrap_or_else(|| OsStr::new(".")))
        .join(".config")
        .join(DIR_NAME)
}

pub fn encode_secret(plain: &str) -> String {
    let mut token = String::from(SECRET_PREFIX);
    token.push_str(&base64::engine::general_purpose::STANDARD.encode(plain.as_bytes()));
    token
}

pub fn decode_secret(token: &str) -> String {
    let Some(encoded) = token.strip_prefix(SECRET_PREFIX) else {
        // Windows DPAPI 令牌（或空值）：本端解不开，按“没有保存密码”处理。
        return String::new();
    };
    match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

fn harden_mode(path: &Path, mode: u32) {
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

pub fn load_config() -> (Config, String) {
    let raw = fs::read_to_string(crate::config::cfg_path()).unwrap_or_default();
    let mut cfg: Config = serde_json::from_str(&raw).unwrap_or_default();
    // 电信出口已停用，旧配置回落到默认校园网。
    if cfg.operator == "telecom" {
        cfg.operator.clear();
    }
    let password = decode_secret(&cfg.password_enc);
    cfg.password_enc = String::new();
    (cfg, password)
}

pub fn save_config(cfg: &Config, password: &str) -> io::Result<()> {
    if cfg.account.trim().is_empty() || password.is_empty() {
        return Err(io::Error::other("请填写账号和密码"));
    }
    let mut stored = cfg.clone();
    stored.password_enc = encode_secret(password);
    let json = serde_json::to_string_pretty(&stored)?;
    crate::config::atomic_write(&crate::config::cfg_path(), &json)?;
    // 原子替换后新文件是 0644，密码在里面，必须立刻收到 0600。
    harden_mode(&crate::config::cfg_path(), 0o600);
    if let Some(dir) = app_dir().parent() {
        harden_mode(dir, 0o700);
    }
    harden_mode(&app_dir(), 0o700);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 密码标记能来回还原() {
        assert_eq!(decode_secret(&encode_secret("p@ss:word")), "p@ss:word");
        assert_eq!(encode_secret("abc"), "plain-v1:YWJj");
    }

    #[test]
    fn 别的平台的密码令牌解不开时按未保存处理() {
        assert_eq!(decode_secret("AQAAANCMnd8BFdERjHoAwE/Cl+sBAAAA"), "");
        assert_eq!(decode_secret(""), "");
        assert_eq!(decode_secret("plain-v1:这不是base64"), "");
    }

    #[test]
    fn 配置目录优先xdg再回落home() {
        assert_eq!(
            config_dir(Some(OsStr::new("/tmp/xdg")), Some(OsStr::new("/home/u"))),
            PathBuf::from("/tmp/xdg/snnulogin")
        );
        // 相对路径的 XDG_CONFIG_HOME 不合规范，按未设置处理
        assert_eq!(
            config_dir(Some(OsStr::new("relative")), Some(OsStr::new("/home/u"))),
            PathBuf::from("/home/u/.config/snnulogin")
        );
        assert_eq!(
            config_dir(None, Some(OsStr::new("/home/u"))),
            PathBuf::from("/home/u/.config/snnulogin")
        );
    }
}
