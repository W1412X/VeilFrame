#!/usr/bin/env bash
# 列出 Debian 池中 libwebkit2gtk-4.1-dev 的全部 amd64 版本（看是否有 deb11 / bullseye 版本）
curl -s -m 120 http://mirrors.aliyun.com/debian/pool/main/w/webkit2gtk/ \
  | grep -o 'href="[^"]*\.deb"' \
  | grep 'libwebkit2gtk-4.1-dev_' \
  | grep 'amd64' \
  | sed 's/href="//; s/"$//'
