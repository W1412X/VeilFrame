#!/usr/bin/env bash
# 列出某个 crate 在 rsproxy 镜像上以指定前缀开头的全部版本，例：query-versions.sh opencv 0.93
name="$1"; prefix="${2:-}"
case ${#name} in
  1) path="1/$name" ;;
  2) path="2/$name" ;;
  3) path="3/${name:0:1}/$name" ;;
  *) path="${name:0:2}/${name:2:2}/$name" ;;
esac
curl -s -m 30 "https://rsproxy.cn/index/$path" | python3 -c '
import json, sys
prefix = sys.argv[1]
v = [json.loads(l)["vers"] for l in sys.stdin if l.strip()]
hits = [x for x in v if x.startswith(prefix)]
print(" ".join(hits) if hits else "(none with prefix " + prefix + ")")
' "$prefix"
