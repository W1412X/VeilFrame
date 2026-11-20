#!/usr/bin/env bash
# 汇总测试日志：每个 crate 的测试数量，以及失败项
log="${1:-/tmp/vf-pure.log}"
echo "--- results per test binary (non-empty only)"
grep -E "Running|test result:" "$log" | awk '
  /Running/ { name=$0; next }
  /test result:/ { if ($4 != "0") print name " => " $0 }
' | sed 's/^ *//'
echo "--- failures"
grep -E "FAILED|panicked" "$log" | head -n 20
echo "--- compile errors"
grep -E "^error" "$log" | head -n 10
echo "--- finished"
grep -c "DONE" "$log"
