#!/usr/bin/env bash
# 从清华 PyPI 镜像取得 Linux 版 ONNX Runtime 动态库（只取 .so，不安装 Python 包）
set -euo pipefail
export PATH="$HOME/.local/bin:$PATH"
ORT_VER="${ORT_VER:-1.20.1}"
DEST="$HOME/opt/onnxruntime"
WHL_DIR="$HOME/vf-deps/whl"
mkdir -p "$WHL_DIR" "$DEST/lib"

python3 -m pip download -q --no-deps \
  -i https://pypi.tuna.tsinghua.edu.cn/simple --trusted-host pypi.tuna.tsinghua.edu.cn \
  --only-binary=:all: --platform manylinux_2_17_x86_64 --python-version 3.10 \
  -d "$WHL_DIR" "onnxruntime==$ORT_VER"

WHL=$(ls "$WHL_DIR"/onnxruntime-"$ORT_VER"-*.whl | head -1)
echo "wheel: $WHL"
TMP=$(mktemp -d)
python3 -c "import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "$WHL" "$TMP"
cp -f "$TMP"/onnxruntime/capi/libonnxruntime.so* "$DEST/lib/"
cp -f "$TMP"/onnxruntime/capi/libonnxruntime_providers_shared.so "$DEST/lib/" 2>/dev/null || true
ls -la "$DEST/lib"
