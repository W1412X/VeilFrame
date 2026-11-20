#!/usr/bin/env bash
# 只测试不依赖 OpenCV / ONNX Runtime 的 crate（OpenCV 未编译完成时使用）
set -euo pipefail
source "$HOME/.cargo/env" 2>/dev/null || true
source /mnt/d/self/VeilFrame/scripts/wsl/env.sh
cd /mnt/d/self/VeilFrame
cargo test -p vf-project -p vf-core -p vf-fusion -p vf-queue -p vf-cache -p vf-text -p vf-media "$@"
