#!/usr/bin/env bash
# ============================================================
# 脚本用途：Linux deb 打包
#
# 层级：packaging/linux
# 调用方：CI linux job；本地 Linux 可直接运行
#
# 约定：安装到 /opt/rdc，桌面入口 /usr/share/applications/rdc.desktop。
# 依赖只列运行时必需库；缺失时 apt 会明确报错，避免"装了但跑不起来"。
# ============================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
VERSION="${VERSION:-$(grep -m1 'version = ' "$REPO_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')}"
BUNDLE="$REPO_ROOT/app/build/linux/x64/release/bundle"
DEB_ROOT="$DIST_DIR/deb-root"
mkdir -p "$DIST_DIR"

echo "[packaging/linux/deb] 版本 $VERSION"

# =====** 构建 =====**
cd "$REPO_ROOT"
cargo build --release -p rdc-bridge -p rdc-cli
cd "$REPO_ROOT/app"
flutter build linux --release

if [ ! -d "$BUNDLE" ]; then
  echo "[packaging/linux/deb] 未找到 $BUNDLE" >&2
  exit 1
fi

# =====** 目录布局 =====**
rm -rf "$DEB_ROOT"
mkdir -p "$DEB_ROOT/DEBIAN" "$DEB_ROOT/opt/rdc" "$DEB_ROOT/usr/share/applications"
cp -R "$BUNDLE/." "$DEB_ROOT/opt/rdc/"
cp "$REPO_ROOT/target/release/librdc_bridge.so" "$DEB_ROOT/opt/rdc/librdc_bridge.so"
cp "$REPO_ROOT/target/release/rdc-cli" "$DEB_ROOT/opt/rdc/rdc-cli"
install -m 0644 "$REPO_ROOT/packaging/linux/rdc.desktop" "$DEB_ROOT/usr/share/applications/rdc.desktop"

# =====** 控制文件 =====**
INSTALLED_SIZE="$(du -sk "$DEB_ROOT" | cut -f1)"
cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: rdc
Version: $VERSION
Section: net
Priority: optional
Architecture: amd64
Installed-Size: $INSTALLED_SIZE
Maintainer: RDC Team <noreply@example.com>
Description: RDC remote desktop control
 Rust core + Flutter UI remote desktop client (S1 skeleton build).
Depends: libgtk-3-0, libblkid1, liblzma5, libstdc++6
EOF

# =====** 构建 =====**
DEB="$DIST_DIR/RDC-$VERSION-amd64.deb"
dpkg-deb --build --root-owner-group "$DEB_ROOT" "$DEB"
sha256sum "$DEB" | sed 's# .*/#  #' > "$DEB.sha256"
echo "[packaging/linux/deb] 完成：$DEB"
