#!/bin/bash
# rdc-serve-stop.sh · 干净停止 Linux 被控端（SIGTERM → exit 0，不记为崩溃）
set -u
PIDFILE=${PIDFILE:-$HOME/rdc-serve/run.pid}
if [ ! -f "$PIDFILE" ]; then
  echo "未找到 $PIDFILE（服务可能未运行）" >&2
  exit 1
fi
pid=$(cat "$PIDFILE")
if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
  echo "进程 $pid 不存在" >&2
  rm -f "$PIDFILE"
  exit 1
fi
kill -TERM "$pid"
echo "已发送 SIGTERM 到 $pid"
