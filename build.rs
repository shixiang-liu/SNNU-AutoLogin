use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn executable(value: &std::ffi::OsStr) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_file() {
        return Some(path);
    }
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(&path))
        .find(|path| path.is_file())
}

fn sdk_rc(bin: &Path, arch: &str) -> Option<PathBuf> {
    let direct = bin.join(arch).join("rc.exe");
    if direct.is_file() {
        return Some(direct);
    }
    let mut versions: Vec<_> = std::fs::read_dir(bin)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let version: Vec<u32> = entry
                .file_name()
                .to_str()?
                .split('.')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            let path = entry.path().join(arch).join("rc.exe");
            path.is_file().then_some((version, path))
        })
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    versions.into_iter().next().map(|(_, path)| path)
}

fn find_rc() -> Option<PathBuf> {
    if let Some(value) = env::var_os("RC") {
        return Some(executable(&value).expect("RC 指定的资源编译器不存在"));
    }
    if let Some(path) = executable(std::ffi::OsStr::new("rc.exe")) {
        return Some(path);
    }
    let host = env::var("HOST").unwrap_or_default();
    let arch = if host.starts_with("aarch64") {
        "arm64"
    } else if host.starts_with("i686") {
        "x86"
    } else {
        "x64"
    };
    for key in ["WindowsSdkVerBinPath", "WindowsSdkBinPath"] {
        if let Some(dir) = env::var_os(key) {
            if let Some(path) = sdk_rc(Path::new(&dir), arch) {
                return Some(path);
            }
        }
    }
    if let Some(dir) = env::var_os("WindowsSdkDir") {
        if let Some(path) = sdk_rc(&PathBuf::from(dir).join("bin"), arch) {
            return Some(path);
        }
    }
    for key in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(dir) = env::var_os(key) {
            if let Some(path) = sdk_rc(&PathBuf::from(dir).join("Windows Kits/10/bin"), arch) {
                return Some(path);
            }
        }
    }
    None
}

fn main() {
    for file in ["app.rc", "icon.ico", "app.manifest", "Cargo.toml"] {
        println!("cargo:rerun-if-changed={file}");
    }
    for key in [
        "RC",
        "PATH",
        "WindowsSdkVerBinPath",
        "WindowsSdkBinPath",
        "WindowsSdkDir",
        "ProgramFiles(x86)",
        "ProgramFiles",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = find_rc().expect("找不到 rc.exe，请安装 Windows SDK，或通过 RC 环境变量指定完整路径");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let package_version = env::var("CARGO_PKG_VERSION").unwrap();
    let version = if env::var("CARGO_PKG_VERSION_PRE")
        .unwrap_or_default()
        .is_empty()
    {
        format!("{package_version}.0")
    } else {
        package_version
    };
    // VERSIONINFO numeric fields accept only four integers, not a SemVer suffix.
    let numeric_version = format!(
        "{},{},{},0",
        env::var("CARGO_PKG_VERSION_MAJOR").unwrap(),
        env::var("CARGO_PKG_VERSION_MINOR").unwrap(),
        env::var("CARGO_PKG_VERSION_PATCH").unwrap()
    );
    let source = std::fs::read_to_string(root.join("app.rc"))
        .expect("无法读取 app.rc")
        .replace("@VERSION_COMMA@", &numeric_version)
        .replace("@VERSION_DOT@", &version);
    let generated = out.join("app.rc");
    let resource = out.join("app.res");
    std::fs::write(&generated, source).expect("无法写入版本资源");
    let status = Command::new(rc)
        .current_dir(&root)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&generated)
        .status()
        .expect("无法运行资源编译器");
    assert!(status.success(), "Windows 资源编译失败");
    println!("cargo:rustc-link-arg-bin=snnulogin={}", resource.display());
}
