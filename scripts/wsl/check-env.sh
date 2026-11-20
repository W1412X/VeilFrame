#!/usr/bin/env bash
# 检查 WSL 开发环境：权限、GPU 直通、apt 源、国内镜像连通性
echo "--- sudo"
if sudo -n true 2>/dev/null; then echo "sudo: passwordless OK"; else echo "sudo: needs password"; fi

echo "--- gpu"
nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader 2>&1 | head -2

echo "--- apt sources"
grep -hv '^#' /etc/apt/sources.list 2>/dev/null | grep -v '^$' | head -3
ls /etc/apt/sources.list.d/ 2>/dev/null

echo "--- mirrors"
for u in https://mirrors.tuna.tsinghua.edu.cn/ubuntu/ https://rsproxy.cn https://registry.npmmirror.com https://pypi.tuna.tsinghua.edu.cn/simple/; do
  code=$(curl -s -o /dev/null -m 10 -w '%{http_code}' "$u")
  echo "$code  $u"
done
