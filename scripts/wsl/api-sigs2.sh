#!/usr/bin/env bash
# 第二轮签名查询：ort（Session::builder、Tensor::from_array、commit）与 opencv（find_homography、add）
REG="$HOME/.cargo/registry/src"
ORT=$(ls -d "$REG"/*/ort-2.0.0-rc.15 2>/dev/null | head -n 1)
OUT=$(ls -d "$HOME"/vf-target/debug/build/opencv-*/out/opencv 2>/dev/null | head -n 1)
echo "--- ort commit-like methods"
grep -rn "fn commit(" "$ORT/src" 2>/dev/null | head -n 5
echo "--- ort init_from doc"
grep -n -B2 -A6 "pub fn init_from" "$ORT/src/environment.rs" | head -n 14
echo "--- ort Session::builder"
grep -rn "pub fn builder" "$ORT/src/session" 2>/dev/null | head -n 3
echo "--- ort Tensor::from_array"
grep -rn "pub fn from_array" "$ORT/src/value" 2>/dev/null | head -n 4
echo "--- ort try_extract_tensor"
grep -rn "fn try_extract_tensor" "$ORT/src/value" 2>/dev/null | head -n 3
echo "--- opencv find_homography / add / perspective_transform"
grep -n "fn find_homography(" "$OUT"/calib3d.rs 2>/dev/null | head -n 2
grep -n "fn add(" "$OUT"/features2d.rs 2>/dev/null | head -n 3
grep -n "fn perspective_transform(" "$OUT"/core.rs "$OUT"/*.rs 2>/dev/null | head -n 2
