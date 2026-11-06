#!/usr/bin/env bash
# ============================================================
# 脚本用途：iOS xcarchive 打包（无签名，需证书时才产出 ipa）
#
# 层级：packaging/ios
# 调用方：CI macos job；本地需 Xcode 与 flutter_rust_bridge 静态库
#
# 现状（S1）：Rust 以 staticlib 形式产出，但尚未在 Xcode 工程中配置链接参数，
#   因此本脚本先验证 xcarchive 可构建；真机运行所需静态链接需在 S2 处理，
#   记录于 docs/决策待办.md（iOS 签名与静态库集成）。
# 凭据：无证书时使用 --no-codesign；有证书时仍保留手动签名步骤给发布负责人。
# ============================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
APP_DIR="$REPO_ROOT/app"
VERSION="${VERSION:-$(grep -m1 'version = ' "$REPO_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')}"
mkdir -p "$DIST_DIR"

echo "[packaging/ios] 版本 $VERSION"

# =====** 构建 iOS staticlib =====**
cd "$REPO_ROOT"
rustup target add aarch64-apple-ios 2>/dev/null || true
cargo build --release -p rdc-bridge --target aarch64-apple-ios

# =====** Flutter 无签名归档 =====**
cd "$APP_DIR"
if [ -n "${IOS_TEAM_ID:-}" ]; then
  echo "[packaging/ios] 检测到 IOS_TEAM_ID，尝试正式签名（仍需证书与描述文件）"
  flutter build ipa --release --export-method ad-hoc || {
    echo "[packaging/ios] 正式签名失败，降级为无签名 xcarchive（日志已说明）" >&2
    flutter build ipa --release --no-codesign
  }
else
  echo "[packaging/ios] 未提供 IOS_TEAM_ID/证书，降级为无签名 xcarchive" >&2
  flutter build ipa --release --no-codesign
fi

ARCHIVE="$REPO_ROOT/app/build/ios/archive/RDC.xcarchive"
if [ -d "$ARCHIVE" ]; then
  tar -C "$REPO_ROOT/app/build/ios/archive" -czf "$DIST_DIR/RDC-$VERSION-ios.xcarchive.tar.gz" RDC.xcarchive
  (cd "$DIST_DIR" && sha256sum "RDC-$VERSION-ios.xcarchive.tar.gz" > "RDC-$VERSION-ios.xcarchive.tar.gz.sha256")
  echo "[packaging/ios] 完成：$DIST_DIR/RDC-$VERSION-ios.xcarchive.tar.gz（未签名）"
else
  echo "[packaging/ios] 未找到 xcarchive，请检查 flutter build ipa 日志" >&2
  exit 1
fi
