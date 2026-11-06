#!/usr/bin/env bash
# ============================================================
# 脚本用途：Android APK 打包（cargo-ndk 交叉编译 + Flutter 签名构建）
#
# 层级：packaging/android
# 调用方：CI android job；本地需安装 Android SDK/NDK 与 cargo-ndk
#
# 凭据策略（docs/08 §5）：
#   - 若环境变量 ANDROID_KEYSTORE_BASE64 + ANDROID_KEYSTORE_PASSWORD + ANDROID_KEY_ALIAS 均存在，
#     使用正式 keystore 生成 release APK；
#   - 否则明确打印降级说明并使用 debug 签名，绝不伪装成正式发布包。
# ============================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
VERSION="${VERSION:-$(grep -m1 'version = ' "$REPO_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')}"
APP_DIR="$REPO_ROOT/app"
JNI_DIR="$APP_DIR/android/app/src/main/jniLibs"
mkdir -p "$DIST_DIR"

echo "[packaging/android] 版本 $VERSION"

# =====** 校验工具链 =====**
if ! command -v cargo-ndk >/dev/null 2>&1; then
  echo "[packaging/android] 缺少 cargo-ndk，请执行：cargo install cargo-ndk" >&2
  exit 1
fi
if [ -z "${ANDROID_NDK_HOME:-}${NDK_HOME:-}" ]; then
  echo "[packaging/android] 缺少 ANDROID_NDK_HOME/NDK_HOME，跳过（需 Android NDK）" >&2
  exit 1
fi

# =====** 交叉编译 Rust 动态库 =====**
cd "$REPO_ROOT"
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo ndk -t arm64-v8a -t armeabi-v7a -o "$JNI_DIR" build --release -p rdc-bridge

# =====** 签名配置 =====**
KEYSTORE_FILE="$APP_DIR/android/rdc-release.jks"
if [ -n "${ANDROID_KEYSTORE_BASE64:-}" ] && [ -n "${ANDROID_KEYSTORE_PASSWORD:-}" ] && [ -n "${ANDROID_KEY_ALIAS:-}" ]; then
  echo "$ANDROID_KEYSTORE_BASE64" | base64 -d > "$KEYSTORE_FILE"
  cat > "$APP_DIR/android/key.properties" <<EOF
storeFile=rdc-release.jks
storePassword=$ANDROID_KEYSTORE_PASSWORD
keyAlias=$ANDROID_KEY_ALIAS
keyPassword=${ANDROID_KEY_PASSWORD:-$ANDROID_KEYSTORE_PASSWORD}
EOF
  echo "[packaging/android] 使用正式 keystore 签名"
  BUILD_MODE="release"
else
  echo "[packaging/android] 警告：未提供 ANDROID_KEYSTORE_* 凭据，降级使用 debug 签名；产物不可上架。" >&2
  BUILD_MODE="debug"
fi

# =====** Flutter 构建 =====**
cd "$APP_DIR"
if [ "$BUILD_MODE" = "release" ]; then
  flutter build apk --release --split-per-abi
  SRC_DIR="$APP_DIR/build/app/outputs/flutter-apk"
  for abi in arm64-v8a armeabi-v7a; do
    # Flutter 的 split 产物名固定为 app-<abi>-release.apk。
    cp "$SRC_DIR/app-$abi-release.apk" "$DIST_DIR/RDC-$VERSION-android-$abi.apk"
  done
else
  flutter build apk --debug --split-per-abi
  SRC_DIR="$APP_DIR/build/app/outputs/flutter-apk"
  for abi in arm64-v8a armeabi-v7a; do
    cp "$SRC_DIR/app-$abi-debug.apk" "$DIST_DIR/RDC-$VERSION-android-$abi-debug.apk"
  done
fi

# =====** 校验和 =====**
for f in "$DIST_DIR"/RDC-"$VERSION"-android-*.apk; do
  (cd "$DIST_DIR" && sha256sum "$(basename "$f")" > "$(basename "$f").sha256")
done

# 清理临时 keystore，避免误提交（.gitignore 已忽略 *.jks，这里再兜底）。
rm -f "$KEYSTORE_FILE" "$APP_DIR/android/key.properties"
echo "[packaging/android] 完成（模式 $BUILD_MODE）"
