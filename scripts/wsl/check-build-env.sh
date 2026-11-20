#!/usr/bin/env bash
# 检查 OpenCV / ONNX Runtime / ffmpeg 构建所需工具
echo "--- ffmpeg"; (ffmpeg -version 2>/dev/null | head -1) || echo "no ffmpeg"
echo "--- cmake"; (cmake --version 2>/dev/null | head -1) || echo "no cmake"
echo "--- ninja"; (ninja --version 2>/dev/null) || echo "no ninja"
echo "--- clang / libclang"
(clang --version 2>/dev/null | head -1) || echo "no clang binary"
ls /usr/lib/x86_64-linux-gnu/libclang*.so* /usr/lib/llvm-*/lib/libclang*.so* 2>/dev/null | head -3 || echo "no libclang"
echo "--- pkg-config"; (pkg-config --version 2>/dev/null) || echo "no pkg-config"
echo "--- python / pip"
python3 --version; (python3 -m pip --version 2>/dev/null) || echo "no pip for python3"
echo "--- opencv source"
ls -la "$HOME/vf-deps/" 2>/dev/null
echo "--- cores / ram"
nproc; free -h | head -2
echo "--- 22.04 webkit"
wsl -d Ubuntu-22.04 -- bash -c 'dpkg -l | grep -c "libwebkit2gtk-4.1-dev"' 2>/dev/null || echo "cannot query 22.04"
