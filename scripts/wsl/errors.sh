#!/usr/bin/env bash
# 列出编译错误及其位置（error 行与紧随其后的 --> 行）
log="${1:-/tmp/vf-full.log}"
grep -A3 -E "^error(\[E[0-9]+\])?:" "$log" | grep -E "^error|-->" | head -n "${2:-60}"
