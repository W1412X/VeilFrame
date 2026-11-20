#!/usr/bin/env bash
# 列出 hf-mirror 上某个模型仓库的文件（path、大小），用于找 ONNX 文件
repo="$1"
curl -s -m 60 "https://hf-mirror.com/api/models/$repo/tree/main" | python3 -c '
import json, sys
try:
    for f in json.load(sys.stdin):
        print(f.get("type", "?"), f.get("path"), f.get("size", ""))
except Exception as e:
    print("(no listing)", e)
'
