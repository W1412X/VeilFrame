#!/usr/bin/env bash
# 在 Ubuntu-22.04 中检查 / 构建 / 运行 Tauri 外壳。
#   bash tauri-22.sh check    只编译检查
#   bash tauri-22.sh build    构建调试版
#   bash tauri-22.sh run      运行（需要先在 Windows 上 npm run dev，前端运行在 localhost:1420）
set -euo pipefail
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
export RUSTUP_DIST_SERVER="${RUSTUP_DIST_SERVER:-https://rsproxy.cn}"
export RUSTUP_UPDATE_ROOT="${RUSTUP_UPDATE_ROOT:-https://rsproxy.cn/rustup}"
export CARGO_TARGET_DIR="$HOME/vf-target-tauri"
cd /mnt/d/self/VeilFrame/app/src-tauri

case "${1:-check}" in
  check) cargo check ;;
  build) cargo build ;;
  run)   cargo run ;;
  *) echo "usage: $0 check|build|run"; exit 2 ;;
esac
