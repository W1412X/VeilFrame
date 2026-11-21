#!/usr/bin/env bash
# 下载 ONNX Runtime 原生动态库到目标目录（NuGet 官方包，三平台齐备）。
# 用法: fetch-ort.sh <linux|windows|macos> <目标目录>
# 版本须满足 ort-sys 的 ORT_API_VERSION=17（运行时 >= 1.17 即可）。
set -euo pipefail

plat="${1:?用法: fetch-ort.sh <linux|windows|macos> <目标目录>}"
dest="${2:?用法: fetch-ort.sh <linux|windows|macos> <目标目录>}"
ver="${ORT_VER:-1.20.1}"

case "$plat" in
  linux)   rid="linux-x64"; need="libonnxruntime.so" ;;
  windows) rid="win-x64";   need="onnxruntime.dll" ;;
  macos)   rid="osx-arm64"; need="libonnxruntime.dylib" ;;
  *) echo "未知平台: $plat" >&2; exit 2 ;;
esac

mkdir -p "$dest"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# --retry-all-errors 覆盖瞬断（连接重置/SSL 中断），这类错误默认不会被 --retry 重试。
curl -sfL --retry 5 --retry-delay 3 --retry-all-errors --connect-timeout 20 \
  -o "$tmp/ort.nupkg" \
  "https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime/$ver"
test -s "$tmp/ort.nupkg" || { echo "下载 ONNX Runtime 包为空" >&2; exit 1; }

# 选一个真实可用的 Python，跳过 Windows 的 Microsoft Store 占位别名。
py=""
for c in python3 python; do
  p="$(command -v "$c" 2>/dev/null || true)"
  [ -n "$p" ] || continue
  case "$p" in *WindowsApps*) continue ;; esac
  py="$c"
  break
done
[ -n "$py" ] || { echo "找不到可用的 Python 解释器" >&2; exit 1; }
"$py" - "$tmp/ort.nupkg" "$rid" "$dest" <<'PY'
import os
import shutil
import sys
import zipfile

nupkg, rid, dest = sys.argv[1:4]
z = zipfile.ZipFile(nupkg)
picked = []
for name in z.namelist():
    prefix = f"runtimes/{rid}/native/"
    if not name.startswith(prefix):
        continue
    base = os.path.basename(name)
    if base.endswith((".so", ".dll", ".dylib")):
        with z.open(name) as src, open(os.path.join(dest, base), "wb") as out:
            shutil.copyfileobj(src, out)
        picked.append(base)
print("ORT 原生库:", ", ".join(picked) or "(空)")
assert picked, f"NuGet 包内没有 {rid} 的原生库"
PY

test -f "$dest/$need" || { echo "缺少 $dest/$need" >&2; exit 1; }
echo "ORT $ver 就绪: $dest/$need"
