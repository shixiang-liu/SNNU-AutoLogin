#!/usr/bin/env bash
# 打包 Linux 无头版，产出两个可以各自独立使用的包：
#   ① tar.gz：二进制 + README + 安装说明 + systemd 用户单元（解压即用）
#   ② .deb  ：/usr/bin/snnulogin + /usr/lib/systemd/user/snnulogin.service + control
# 两个产物的 SHA256 一起写进 dist/linux/SHA256SUMS。
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$version" ] || { echo "读不到 Cargo.toml 里的 version" >&2; exit 1; }

arch=${ARCH:-$(dpkg --print-architecture 2>/dev/null || uname -m)}
out="$root/dist/linux"
unit="packaging/linux/snnulogin.service"
doc="packaging/linux/INSTALL.md"
maintainer=${MAINTAINER:-"SNNU-AutoLogin <snnu-autologin@users.noreply.github.com>"}

bin=${SNNU_BIN:-}
if [ -z "$bin" ]; then
  command -v cargo >/dev/null || {
    echo "找不到 cargo：先 cargo build --release --locked，再用 SNNU_BIN=<路径> 指定二进制" >&2
    exit 1
  }
  cargo build --release --locked
  bin="${CARGO_TARGET_DIR:-target}/release/snnulogin"
fi
[ -x "$bin" ] || { echo "二进制不存在或不可执行：$bin" >&2; exit 1; }

actual=$("$bin" --version | sed 's/.*v//')
[ "$actual" = "$version" ] || { echo "二进制版本 $actual 与 Cargo.toml 的 $version 不一致" >&2; exit 1; }

# 在本地文件系统里组装：仓库可能位于 drvfs/网络盘（文件权限一律被读成 777），
# 而 dpkg-deb 要求 control 目录权限必须是 0755，tar 也会把 777 原样打包进去。
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

rm -rf "$out"
mkdir -p "$out"

# ① tar.gz
name="snnulogin-$version-linux-$arch"
mkdir -p "$stage/$name/systemd"
install -Dm755 "$bin" "$stage/$name/snnulogin"
install -Dm644 README.md "$stage/$name/README.md"
install -Dm644 "$doc" "$stage/$name/INSTALL.md"
install -Dm644 "$unit" "$stage/$name/systemd/snnulogin.service"
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"

# ② deb
pkgdir="$stage/pkg"
mkdir -p "$pkgdir/DEBIAN" "$pkgdir/usr/bin" "$pkgdir/usr/lib/systemd/user" "$pkgdir/usr/share/doc/snnulogin"
install -Dm755 "$bin" "$pkgdir/usr/bin/snnulogin"
install -Dm644 "$unit" "$pkgdir/usr/lib/systemd/user/snnulogin.service"
install -Dm644 README.md "$pkgdir/usr/share/doc/snnulogin/README.md"
install -Dm644 LICENSE "$pkgdir/usr/share/doc/snnulogin/copyright"
chmod 0755 "$pkgdir/DEBIAN"
size=$(du -sk "$pkgdir/usr" | cut -f1)
cat > "$pkgdir/DEBIAN/control" <<EOF
Package: snnulogin
Version: $version
Section: net
Priority: optional
Architecture: $arch
Maintainer: $maintainer
Installed-Size: $size
Recommends: network-manager
Description: 陕师大校园网自动登录工具（Linux 无头版）
 常驻进程按周期探测校园网门户，未认证时自动提交登录，断线后自动重连。
 只访问校园网 HTTP 门户，不需要 root 权限，也不改动系统网络设置。
 Wi-Fi 自愈为可选能力：系统装有 NetworkManager(nmcli) 时才会启用。
EOF
dpkg-deb --root-owner-group --build "$pkgdir" "$out/snnulogin_${version}_${arch}.deb"

( cd "$out" && sha256sum ./*.tar.gz ./*.deb > SHA256SUMS && cat SHA256SUMS )
echo "产物目录：$out"
