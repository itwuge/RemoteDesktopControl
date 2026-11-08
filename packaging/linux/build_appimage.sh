#!/usr/bin/env bash
# ============================================================
# 脚本用途：Linux AppImage 打包（缺失 appimagetool 时降级为 tar.gz）
#
# 层级：packaging/linux
# 调用方：CI linux job；本地 Linux 可直接运行
# ============================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
VERSION="${VERSION:-$(grep -m1 'version = ' "$REPO_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')}"
BUNDLE="$REPO_ROOT/app/build/linux/x64/release/bundle"
mkdir -p "$DIST_DIR"

echo "[packaging/linux/appimage] 版本 $VERSION"

# =====** 构建 =====**
cd "$REPO_ROOT"
cargo build --release -p rdc-bridge -p rdc-cli
cd "$REPO_ROOT/app"
flutter build linux --release

if [ ! -d "$BUNDLE" ]; then
  echo "[packaging/linux/appimage] 未找到 $BUNDLE" >&2
  exit 1
fi

# =====** 动态库与 CLI =====**
# Dart 侧按可执行文件同目录查找 librdc_bridge.so（见 rust_gateway.dart）。
cp "$REPO_ROOT/target/release/librdc_bridge.so" "$BUNDLE/librdc_bridge.so"
cp "$REPO_ROOT/target/release/rdc-cli" "$BUNDLE/rdc-cli"
COMMIT="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
printf 'version=%s\ncommit=%s\n' "$VERSION" "$COMMIT" > "$BUNDLE/BUILD_INFO.txt"

# =====** AppDir =====**
APPDIR="$DIST_DIR/RDC.AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp -R "$BUNDLE/." "$APPDIR/usr/bin/"
install -m 0644 "$REPO_ROOT/packaging/linux/rdc.desktop" "$APPDIR/rdc.desktop"
install -m 0644 "$REPO_ROOT/packaging/linux/rdc.desktop" "$APPDIR/usr/share/applications/rdc.desktop"
# .desktop 里写了 Icon=rdc，appimagetool 找不到同名图标会直接 exit 1（不是警告），
# 所以这里必须有图标：优先 256x256 专用图，退化到 Android 启动图标（xxxhdpi 192px）。
ICON_SRC=""
for candidate in \
  "$REPO_ROOT/app/web/icons/Icon-256.png" \
  "$REPO_ROOT/app/android/app/src/main/res/mipmap-xxxhdpi/ic_launcher.png" \
  "$REPO_ROOT/app/android/app/src/main/res/mipmap-xxhdpi/ic_launcher.png"; do
  if [ -f "$candidate" ]; then
    ICON_SRC="$candidate"
    break
  fi
done
if [ -z "$ICON_SRC" ]; then
  echo "[packaging/linux/appimage] 错误：找不到任何可用图标源，appimagetool 无法打包" >&2
  exit 1
fi
install -m 0644 "$ICON_SRC" "$APPDIR/usr/share/icons/hicolor/256x256/apps/rdc.png"
install -m 0644 "$ICON_SRC" "$APPDIR/rdc.png"
cat > "$APPDIR/AppRun" <<'EOF'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")"
exec "$HERE/usr/bin/rdc" "$@"
EOF
chmod +x "$APPDIR/AppRun"

# =====** appimagetool（下载失败则降级 tar.gz） =====**
APPIMAGE="$DIST_DIR/RDC-$VERSION-x86_64.AppImage"
TOOL="$DIST_DIR/appimagetool"
if [ ! -x "$TOOL" ]; then
  if curl -fsSL -o "$TOOL" https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage; then
    chmod +x "$TOOL"
  else
    echo "[packaging/linux/appimage] 警告：下载 appimagetool 失败，降级为 tar.gz" >&2
    tar -C "$APPDIR/usr/bin" -czf "$DIST_DIR/RDC-$VERSION-linux-x64.tar.gz" .
    echo "[packaging/linux/appimage] 降级产物：$DIST_DIR/RDC-$VERSION-linux-x64.tar.gz"
    exit 0
  fi
fi

ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "$APPIMAGE" 2>/dev/null || \
  ARCH=x86_64 "$TOOL" "$APPDIR" "$APPIMAGE"

sha256sum "$APPIMAGE" | sed 's# .*/#  #' > "$APPIMAGE.sha256"
echo "[packaging/linux/appimage] 完成：$APPIMAGE"
