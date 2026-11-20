#!/usr/bin/env bash
# 显示失败的测试、panic 信息与断言左右值
log="${1:-/tmp/vf-full.log}"
grep -E "FAILED|panicked|left:|right:|assertion|test result" "$log" | head -n 40
echo "--- tail"
tail -n 1 "$log"
