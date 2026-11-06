#!/usr/bin/env bash
# ============================================================
# 脚本用途：macOS dmg 打包（ad-hoc 签名，不公证）
#
# 层级：packaging/macos
# 调用方：CI macos job；本地 macOS 可直接运行
#
# 说明：CI 没有 Apple 开发者证书，按 docs/08 §5 使用 `codesign -s -`（ad-hoc）；
#       首次打开需右键「打开」，脚本会打印提示，不隐藏体验代价。
# ============================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_PATH="$REPO_ROOT/app/build/macos/Build/Products/Release/RDC.app"
DIST_DIR="$REPO_ROOT/dist"
VERSION="${VERSION:-$(grep -m1 'version = ' "$REPO_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')}"
mkdir -p "$DIST_DIR"

echo "[packaging/macos] 版本 $VERSION"

# =====** Rust 产物 =====**
cd "$REPO_ROOT"
cargo build --release -p rdc-bridge -p rdc-cli

# =====** Flutter 构建 =====**
cd "$REPO_ROOT/app"
flutter build macos --release

if [ ! -d "$APP_PATH" ]; then
  echo "[packaging/macos] 未找到 $APP_PATH" >&2
  exit 1
fi

# =====** 动态库与 CLI 布局 =====**
# Dart 侧按 Contents/Frameworks 查找 librdc_bridge.dylib（见 rust_gateway.dart）。
FRAMEWORKS="$APP_PATH/Contents/Frameworks"
mkdir -p "$FRAMEWORKS"
cp "$REPO_ROOT/target/release/librdc_bridge.dylib" "$FRAMEWORKS/librdc_bridge.dylib"
mkdir -p "$APP_PATH/Contents/Resources"
cp "$REPO_ROOT/target/release/rdc-cli" "$APP_PATH/Contents/Resources/rdc-cli"

# =====** 构建信息 =====**
COMMIT="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
printf 'version=%s\ncommit=%s\n' "$VERSION" "$COMMIT" > "$APP_PATH/Contents/Resources/BUILD_INFO.txt"

# =====** ad-hoc 签名 =====**
# 先签内部 dylib 再签 app，顺序错误会导致嵌套签名校验失败。
codesign --force --sign - "$FRAMEWORKS/librdc_bridge.dylib"
codesign --force --deep --sign - "$APP_PATH"

# =====** dmg =====**
STAGE_DIR="$DIST_DIR/dmg-stage"
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR"
cp -R "$APP_PATH" "$STAGE_DIR/"
ln -s /Applications "$STAGE_DIR/Applications"

DMG="$DIST_DIR/RDC-$VERSION-macos.dmg"
rm -f "$DMG"
hdiutil create -volname "RDC" -srcfolder "$STAGE_DIR" -ov -format UDZO "$DMG"

# =====** 校验和 =====**
shasum -a 256 "$DMG" | awk '{print $1"  "$2}' > "$DMG.sha256"

echo "[packaging/macos] 完成：$DMG"
echo "[packaging/macos] 提示：未公证，首次打开需右键「打开」，或执行 xattr -d com.apple.quarantine \"$DMG\""
