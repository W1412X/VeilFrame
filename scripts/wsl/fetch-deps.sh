#!/usr/bin/env bash
# 下载外部依赖到 ~/vf-deps（不在仓库内，避免把大文件提交进 git）。
# 已存在且非空的文件会跳过；下载使用断点续传。
set -uo pipefail
DEPS="$HOME/vf-deps"
mkdir -p "$DEPS"
cd "$DEPS" || exit 1

fetch() {
  local url="$1" out="$2"
  if [ -s "$out" ]; then
    echo "skip (exists): $out"
    return 0
  fi
  echo "download: $url"
  # 失败时保留部分文件，下次用 -C - 续传；最多重试 5 轮
  for attempt in 1 2 3 4 5; do
    if curl -L -sS --fail --connect-timeout 30 --speed-time 120 --speed-limit 1000 -C - -o "$out" "$url"; then
      echo "ok: $out ($(du -h "$out" | cut -f1))"
      return 0
    fi
    echo "attempt $attempt failed, resuming..."
    sleep 3
  done
  echo "FAILED: $url"
  return 1
}

fetch https://github.com/opencv/opencv/archive/refs/tags/4.10.0.zip opencv-4.10.0.zip
fetch https://github.com/microsoft/onnxruntime/releases/download/v1.20.1/onnxruntime-linux-x64-1.20.1.tgz onnxruntime-linux-x64-1.20.1.tgz
fetch https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl-shared.tar.xz ffmpeg-master-latest-linux64-gpl-shared.tar.xz

echo "--- done"
ls -la "$DEPS"
