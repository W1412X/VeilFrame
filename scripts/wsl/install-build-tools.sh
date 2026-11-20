#!/usr/bin/env bash
# 通过清华 PyPI 镜像安装用户级构建工具（无需 sudo）：cmake、ninja、libclang（供 bindgen 使用）
set -euo pipefail
export PATH="$HOME/.local/bin:$PATH"
python3 -m pip install --user -q -i https://pypi.tuna.tsinghua.edu.cn/simple --trusted-host pypi.tuna.tsinghua.edu.cn cmake ninja libclang
echo "cmake: $(cmake --version | head -1)"
echo "ninja: $(ninja --version)"
python3 -c "import clang.cindex as c; print('libclang python bindings ok')" 2>/dev/null || true
find "$HOME/.local" -name 'libclang*.so*' 2>/dev/null | head -3
