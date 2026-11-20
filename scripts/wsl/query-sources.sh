#!/usr/bin/env bash
# 查询候选模型仓库的文件列表（找 ONNX 文件）以及 ort crate 版本
list_files() {
  local repo="$1"
  echo "--- files in $repo"
  curl -s -m 30 "https://hf-mirror.com/api/models/$repo/tree/main" | python3 -c '
import json, sys
try:
    for f in json.load(sys.stdin):
        print("  ", f.get("path"), f.get("size", ""))
except Exception as e:
    print("   (no listing):", e)
'
}

list_files PaddlePaddle/PP-OCRv5_mobile_det
list_files PaddlePaddle/PP-OCRv5_mobile_rec
list_files breezedeus/cnocr-ppocr-ch_PP-OCRv5
list_files breezedeus/cnstd-ppocr-ch_PP-OCRv5_det

echo "--- ort crate versions (sparse index path for 3-letter name: 3/o/ort)"
curl -s -m 30 https://rsproxy.cn/index/3/o/ort | python3 -c '
import json, sys
lines = [l for l in sys.stdin.read().splitlines() if l.strip()]
vers = [json.loads(l)["vers"] for l in lines]
print(" ".join(vers[-15:]) if vers else "(no versions)")
'
