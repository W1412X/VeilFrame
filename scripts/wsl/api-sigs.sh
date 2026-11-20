#!/usr/bin/env bash
# 从已下载的 crate 源码与构建输出中查找关键函数签名（ort 与 opencv）
REG="$HOME/.cargo/registry/src"
OUT=$(ls -d "$HOME"/vf-target/debug/build/opencv-*/out/opencv 2>/dev/null | head -n 1)
echo "--- ort EnvironmentBuilder methods"
grep -n "pub fn " "$REG"/*/ort-2.0.0-rc.15/src/environment.rs 2>/dev/null | sed -n '1,40p' | grep -iE "commit|with_name|build|fn " | head -n 12
echo "--- ort session builder commit_from_file"
grep -rn "pub fn commit_from_file" "$REG"/*/ort-2.0.0-rc.15/src 2>/dev/null | head -n 2
echo "--- opencv generated: knn_match / detect_and_compute / find_homography / SIFT create"
grep -n "fn knn_match" "$OUT"/features2d.rs 2>/dev/null | head -n 3
grep -n "fn detect_and_compute" "$OUT"/features2d.rs 2>/dev/null | head -n 4
grep -n "pub fn find_homography" "$OUT"/calib3d.rs 2>/dev/null | head -n 2
grep -n "pub fn create" "$OUT"/features2d.rs 2>/dev/null | head -n 3
echo "--- opencv out dir: $OUT"
