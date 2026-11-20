#!/usr/bin/env bash
# 显示指定 crate 的完整编译错误块（用于看 expected / found 类型）
log="${1:-/tmp/vf-full.log}"; pat="${2:-vf-ocr/src/lib.rs}"
awk -v p="$pat" '
  /^error/ { buf=$0; keep=0; collecting=1; next }
  collecting { buf = buf "\n" $0 }
  /^$/ && collecting { if (index(buf, p)) print buf "\n----"; collecting=0; buf="" }
' "$log" | head -n "${3:-80}"
