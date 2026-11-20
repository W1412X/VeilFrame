#!/usr/bin/env bash
# 在 WSL 中编译并运行 workspace 测试（OpenCV / libclang / ORT 路径由 env.sh 提供）。
# 构建产物放在 WSL 本地目录，避免在 /mnt/d（NTFS）上编译过慢。
set -euo pipefail
source /mnt/d/self/VeilFrame/scripts/wsl/env.sh
cd /mnt/d/self/VeilFrame
cargo test --workspace "$@"
