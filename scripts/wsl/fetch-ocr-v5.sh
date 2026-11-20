#!/usr/bin/env bash
# 后台下载 PP-OCRv5 mobile ONNX（hf-mirror，断点续传）到 D:\vf-models\ocr
set -uo pipefail
DEST=/mnt/d/vf-models/ocr
mkdir -p "$DEST"
fetch() {
  local url="$1" out="$2"
  if [ -s "$out" ]; then echo "skip (exists): $out"; return 0; fi
  echo "download: $url"
  for attempt in 1 2 3 4 5 6; do
    if curl -L -sS --fail --connect-timeout 30 --speed-time 120 --speed-limit 1000 -C - -o "$out" "$url"; then
      echo "ok: $out ($(stat -c %s "$out") bytes)"
      return 0
    fi
    echo "attempt $attempt failed, resuming"
  done
  echo "FAILED: $url"
  return 1
}
fetch https://hf-mirror.com/breezedeus/cnstd-ppocr-ch_PP-OCRv5_det/resolve/main/ch_PP-OCRv5_det_infer.onnx "$DEST/det_v5.onnx"
fetch https://hf-mirror.com/breezedeus/cnocr-ppocr-ch_PP-OCRv5/resolve/main/ch_PP-OCRv5_rec_infer.onnx "$DEST/rec_v5.onnx"
echo "DONE"
