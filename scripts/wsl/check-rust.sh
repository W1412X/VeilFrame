#!/usr/bin/env bash
# 确认 Rust 工具链是否已装（非登录 shell 下 PATH 可能不含 ~/.cargo/bin）
echo "--- cargo binary"
ls -la "$HOME/.cargo/bin/cargo" "$HOME/.cargo/bin/rustc" 2>&1
echo "--- env file"
ls -la "$HOME/.cargo/env" 2>&1
echo "--- versions (via ~/.cargo/bin)"
if [ -x "$HOME/.cargo/bin/rustc" ]; then
  "$HOME/.cargo/bin/rustc" --version
  "$HOME/.cargo/bin/cargo" --version
  "$HOME/.cargo/bin/rustup" show active-toolchain 2>&1 | head -2
  "$HOME/.cargo/bin/rustup" target list --installed 2>&1 | head -5
fi
echo "--- cargo config"
cat "$HOME/.cargo/config.toml" 2>/dev/null || cat "$HOME/.cargo/config" 2>/dev/null || echo "(no cargo config)"
echo "--- PhysScape project"
ls -la "$HOME/Files/PhysScape" 2>&1 | head -20
echo "--- PhysScape Cargo.toml"
cat "$HOME/Files/PhysScape/Cargo.toml" 2>/dev/null || echo "(no Cargo.toml at top level)"
echo "--- rustup env vars"
env | grep -E 'RUSTUP|CARGO' || echo "(none set in this shell)"
