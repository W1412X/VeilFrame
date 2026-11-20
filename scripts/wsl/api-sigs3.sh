#!/usr/bin/env bash
# 第三轮：ort 环境创建方式、OwnedTensorArrayData 的实现；以及 vf-vision/sift.rs 报错行
REG="$HOME/.cargo/registry/src"
ORT=$(ls -d "$REG"/*/ort-2.0.0-rc.15 2>/dev/null | head -n 1)
echo "--- ort environment pub fns"
grep -n "pub fn \|pub(crate) fn " "$ORT/src/environment.rs" | head -n 30
echo "--- OwnedTensorArrayData impls"
grep -rn "impl.*OwnedTensorArrayData" "$ORT/src" 2>/dev/null | head -n 8
echo "--- sift.rs lines 50-60 (vf-vision)"
sed -n '50,60p' /mnt/d/self/VeilFrame/crates/vf-vision/src/sift.rs
echo "--- vf-ocr.rs lines 120-130"
sed -n '120,130p' /mnt/d/self/VeilFrame/crates/vf-ocr/src/lib.rs
