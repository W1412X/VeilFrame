#!/usr/bin/env bash
# 直接从清华 PyPI 镜像的 simple 索引找到 onnxruntime 的 cp310 manylinux x86_64 wheel，
# 用 curl 下载并解压出 libonnxruntime.so（不依赖 pip 的平台标签匹配）。
set -uo pipefail
VER="${ORT_VER:-1.20.1}"
DEST="$HOME/opt/onnxruntime"
TMP="$HOME/vf-deps/ort"
mkdir -p "$DEST/lib" "$TMP"

INDEX=$(curl -s -m 60 https://pypi.tuna.tsinghua.edu.cn/simple/onnxruntime/)
if [ -z "$INDEX" ]; then echo "FAILED: index empty"; exit 1; fi

# 候选：cp310 或 cp311 的 manylinux x86_64 wheel，优先 manylinux_2_27 / 2_17
URL=$(echo "$INDEX" | grep -o "href=\"[^\"]*onnxruntime-$VER-cp31[01]-cp31[01]-manylinux[^\"]*x86_64.whl[^\"]*\"" | head -n 1 | sed 's/^href="//; s/"$//; s/#.*$//')
if [ -z "$URL" ]; then
  echo "FAILED: no cp310/cp311 manylinux wheel for $VER in index"
  echo "$INDEX" | grep -o "onnxruntime-$VER-[^\"<]*" | sort -u | head -n 20
  exit 1
fi
case "$URL" in
  http*) ;;
  *) URL="https://pypi.tuna.tsinghua.edu.cn/simple/onnxruntime/$URL" ;;
esac
WHL="$TMP/$(basename "${URL%%\?*}")"
echo "download: $URL"
curl -L -sS --fail --connect-timeout 30 --speed-time 120 --speed-limit 1000 -C - -o "$WHL" "$URL" || { echo "FAILED: download"; exit 1; }
echo "wheel: $WHL ($(stat -c %s "$WHL") bytes)"

python3 -c 'import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' "$WHL" "$TMP/whl" || { echo "FAILED: unzip"; exit 1; }
cp -f "$TMP"/whl/onnxruntime/capi/libonnxruntime.so* "$DEST/lib/"
REAL=$(ls "$DEST"/lib/libonnxruntime.so.* 2>/dev/null | head -n 1)
[ -n "$REAL" ] && ln -sf "$(basename "$REAL")" "$DEST/lib/libonnxruntime.so"
ls -la "$DEST/lib"
echo "DONE"
