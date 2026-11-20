#!/usr/bin/env bash
# 查询 crate 的最近版本（rsproxy 稀疏索引）
q() {
  local name="$1" path
  case ${#name} in
    1) path="1/$name" ;;
    2) path="2/$name" ;;
    3) path="3/${name:0:1}/$name" ;;
    *) path="${name:0:2}/${name:2:2}/$name" ;;
  esac
  printf '%s: ' "$name"
  curl -s -m 30 "https://rsproxy.cn/index/$path" | python3 -c '
import json, sys
v = [json.loads(l)["vers"] for l in sys.stdin if l.strip()]
print(" ".join(v[-8:]) if v else "(none)")
'
}
for c in "$@"; do q "$c"; done
