#!/usr/bin/env bash
# 检查外部依赖下载源的连通性（HTTP 状态码）
for u in \
  https://github.com \
  https://objects.githubusercontent.com \
  https://huggingface.co \
  https://hf-mirror.com \
  https://modelscope.cn \
  https://mirrors.aliyun.com/ubuntu/ \
  https://files.pythonhosted.org \
  https://pypi.tuna.tsinghua.edu.cn/simple/ ; do
  code=$(curl -s -o /dev/null -m 12 -w '%{http_code}' "$u")
  echo "$code  $u"
done
