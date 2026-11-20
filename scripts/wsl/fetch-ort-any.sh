#!/usr/bin/env bash
# 获取 Linux 版 ONNX Runtime 动态库到 ~/opt/onnxruntime/lib。依次尝试：
#   1) 清华 PyPI 镜像的 cp38 wheel（WSL 的 Python 为 3.8，ort rc.15 的 API 17 与 1.17.x 兼容）
#   2) GitHub 官方 tgz（网络可用时）
# 成功后 libonnxruntime.so 位于 ~/opt/onnxruntime/lib/
set -uo pipefail
VER="${ORT_VER:-1.17.3}"
DEST="$HOME/opt/onnxruntime"
TMP="$HOME/vf-deps/ort"
mkdir -p "$DEST/lib" "$TMP"

have_lib() { ls "$DEST"/lib/libonnxruntime.so* >/dev/null 2>&1; }

if have_lib; then echo "already present"; ls "$DEST/lib"; exit 0; fi

# 1) 清华 PyPI 镜像 wheel（当前解释器 cp38，不指定平台标签）
echo "try pypi mirror: onnxruntime==$VER"
python3 -m pip download -q --no-deps \
  -i https://pypi.tuna.tsinghua.edu.cn/simple --trusted-host pypi.tuna.tsinghua.edu.cn \
  -d "$TMP" "onnxruntime==$VER"
WHL=$(ls "$TMP"/onnxruntime-"$VER"-cp38-*manylinux*x86_64*.whl 2>/dev/null | head -n 1)
if [ -n "$WHL" ]; then
  python3 -c 'import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' "$WHL" "$TMP/whl"
  cp -f "$TMP"/whl/onnxruntime/capi/libonnxruntime.so* "$DEST/lib/"
  echo "installed from pypi wheel: $WHL"
fi

# 2) GitHub 官方 tgz（备用）
if ! have_lib; then
  TGZ="$TMP/onnxruntime-linux-x64-$VER.tgz"
  URL="https://github.com/microsoft/onnxruntime/releases/download/v$VER/onnxruntime-linux-x64-$VER.tgz"
  echo "try github: $URL"
  curl -L -sS --fail --connect-timeout 30 -o "$TGZ" "$URL" && tar -xzf "$TGZ" -C "$TMP" && \
    cp -f "$TMP"/onnxruntime-linux-x64-"$VER"/lib/libonnxruntime.so* "$DEST/lib/" && \
    echo "installed from github"
fi

if have_lib; then
  # 确保有不带版本号的 libonnxruntime.so 便于 ORT_DYLIB_PATH 使用
  REAL=$(ls "$DEST"/lib/libonnxruntime.so.* 2>/dev/null | head -n 1)
  [ -n "$REAL" ] && ln -sf "$(basename "$REAL")" "$DEST/lib/libonnxruntime.so"
  echo "OK"; ls -la "$DEST/lib"
else
  echo "FAILED: no libonnxruntime.so"
fi
echo "DONE"
