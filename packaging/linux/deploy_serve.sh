#!/bin/bash
# deploy_serve.sh · 安装/更新 Linux 被控端常驻服务（S8-5）
#
# 用法：cargo build --release --bin rdc-serve && packaging/linux/deploy_serve.sh
# 安装位置：$HOME/rdc-serve（二进制 + run.sh + stop.sh），
# 自启：$HOME/.config/autostart/rdc-serve.desktop（图形会话，非 systemd 系统服务），
# 配置：$HOME/.config/rdc/serve.conf（已存在则不覆盖），
# 日志：$HOME/.local/state/rdc/serve.log。

set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN_SRC=${BIN_SRC:-$ROOT/target/release/rdc-serve}
INSTALL=${INSTALL:-$HOME/rdc-serve}
STATE=${XDG_STATE_HOME:-$HOME/.local/state}/rdc
CONF_DIR=${XDG_CONFIG_HOME:-$HOME/.config}/rdc
AUTOSTART_DIR=${XDG_CONFIG_HOME:-$HOME/.config}/autostart

if [ ! -x "$BIN_SRC" ]; then
  echo "找不到可执行文件 $BIN_SRC" >&2
  echo "请先运行: cargo build --release -p rdc-linux-e2e --bin rdc-serve" >&2
  exit 1
fi

mkdir -p "$INSTALL" "$STATE" "$CONF_DIR" "$AUTOSTART_DIR"
install -m 0755 "$BIN_SRC" "$INSTALL/rdc-serve"
install -m 0755 "$ROOT/packaging/linux/rdc-serve-run.sh" "$INSTALL/run.sh"
install -m 0755 "$ROOT/packaging/linux/rdc-serve-stop.sh" "$INSTALL/stop.sh"

if [ ! -f "$CONF_DIR/serve.conf" ]; then
  install -m 0600 "$ROOT/packaging/linux/serve.conf.example" "$CONF_DIR/serve.conf"
fi

sed "s|@EXEC@|$INSTALL/run.sh|" "$ROOT/packaging/linux/rdc-serve.desktop" >"$AUTOSTART_DIR/rdc-serve.desktop"
chmod 0644 "$AUTOSTART_DIR/rdc-serve.desktop"

echo "INSTALL_OK bin=$INSTALL/rdc-serve"
echo "autostart=$AUTOSTART_DIR/rdc-serve.desktop"
echo "config=$CONF_DIR/serve.conf log=$STATE/serve.log"
