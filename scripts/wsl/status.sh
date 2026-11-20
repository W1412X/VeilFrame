#!/usr/bin/env bash
# 汇总后台任务状态：纯 crate 测试结果、OpenCV 构建进度、残留进程
echo "--- pure crate tests"
grep -E "test result|FAILED|panicked|^error|error\[|DONE" /tmp/vf-pure.log 2>/dev/null | tail -n 30
echo "--- opencv build (last lines)"
tail -n 3 /tmp/vf-opencv3.log 2>/dev/null
echo "--- opencv build errors"
grep -E "FAILED: |CMake Error|error:" /tmp/vf-opencv3.log 2>/dev/null | head -n 5
echo "--- installed opencv"
ls "$HOME/opt/opencv/lib" 2>/dev/null | head -n 5 || echo "not installed yet"
echo "--- running cargo/cmake/ninja"
pgrep -a -x "cargo|ninja|cmake" 2>/dev/null | head -n 5
