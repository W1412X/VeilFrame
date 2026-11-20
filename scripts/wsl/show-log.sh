#!/usr/bin/env bash
# 显示指定日志的最后 N 行（默认 40）
f="$1"; n="${2:-40}"
if [ -f "$f" ]; then tail -n "$n" "$f"; else echo "missing: $f"; fi
