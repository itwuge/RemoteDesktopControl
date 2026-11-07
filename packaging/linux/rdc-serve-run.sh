#!/bin/bash
# rdc-serve-run.sh · 图形会话内启动 Linux 被控端（S8-5）
#
# 由 ~/.config/autostart/rdc-serve.desktop 调起，也可手动执行。
# 约束：必须运行在图形会话里（需要 DISPLAY/XAUTHORITY），故不使用 systemd 系统级服务。
# 崩溃留痕：非 0 退出写入 serve-crash.log，并自动重启；5 分钟内连续 3 次后放弃。

set -u
DIR=$(cd "$(dirname "$0")" && pwd)
STATE=${XDG_STATE_HOME:-$HOME/.local/state}/rdc
LOG="$STATE/serve-console.log"
CRASH="$STATE/serve-crash.log"
PIDFILE="$DIR/run.pid"
mkdir -p "$STATE"

export DISPLAY="${DISPLAY:-:0}"
if [ -z "${XAUTHORITY:-}" ]; then
  for candidate in "/run/user/$(id -u)/gdm/Xauthority" "$HOME/.Xauthority"; do
    [ -f "$candidate" ] && export XAUTHORITY="$candidate" && break
  done
fi

crashes=0
while :; do
  started=$(date +%s)
  "$DIR/rdc-serve" >>"$LOG" 2>&1 &
  child=$!
  echo "$child" >"$PIDFILE"
  wait "$child"
  rc=$?
  uptime=$(( $(date +%s) - started ))
  if [ "$rc" -eq 0 ]; then
    echo "$(date -Is) SERVE_EXIT rc=0 uptime_s=$uptime clean=1" >>"$CRASH"
    rm -f "$PIDFILE"
    exit 0
  fi
  echo "$(date -Is) SERVE_EXIT rc=$rc uptime_s=$uptime clean=0" >>"$CRASH"
  tail -n 12 "$LOG" >>"$CRASH" 2>/dev/null
  if [ "$uptime" -ge 60 ]; then
    crashes=1
  else
    crashes=$((crashes + 1))
  fi
  if [ "$crashes" -ge 3 ]; then
    echo "$(date -Is) SERVE_GIVEUP crashes=$crashes" >>"$CRASH"
    rm -f "$PIDFILE"
    exit 0
  fi
  sleep 3
done
