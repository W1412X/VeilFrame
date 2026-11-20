#!/usr/bin/env bash
# 查找 webkit2gtk-4.1 的 deb 包列表（阿里云镜像），并检查 Depends 中的 libc6 版本要求
set -uo pipefail
echo "--- ubuntu jammy (22.04) pool listing: webkit2gtk"
curl -s -m 60 http://mirrors.aliyun.com/ubuntu/pool/main/w/webkit2gtk/ | grep -o 'href="[^"]*\.deb"' | grep -E "4\.1|_amd64" | sed 's/href="//;s/"$//' | grep -E "libwebkit2gtk-4.1-(0|dev)_" | tail -n 6

echo "--- debian bullseye-backports (glibc 2.31, same as Ubuntu 20.04) pool listing"
curl -s -m 60 http://mirrors.aliyun.com/debian/pool/main/w/webkit2gtk/ | grep -o 'href="[^"]*\.deb"' | grep -E "libwebkit2gtk-4.1-(0|dev)_.*amd64" | sed 's/href="//;s/"$//' | tail -n 6
curl -s -m 60 http://mirrors.aliyun.com/debian/pool/main/w/webkit2gtk/ | grep -o 'href="[^"]*\.deb"' | grep -c amd64

echo "--- bullseye-backports Packages index: Depends of libwebkit2gtk-4.1-dev"
curl -s -m 180 http://mirrors.aliyun.com/debian/dists/bullseye-backports/main/binary-amd64/Packages.gz | gunzip -c 2>/dev/null | awk 'BEGIN{RS="";FS="\n"} /Package: libwebkit2gtk-4.1-dev\n/ {print}' | grep -E "^(Version|Depends|Filename):" | head -n 4
