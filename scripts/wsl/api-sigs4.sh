#!/usr/bin/env bash
# 第四轮：BFMatcher::new、box_points、SIFT::create_def 的返回类型
OUT=$(ls -d "$HOME"/vf-target/debug/build/opencv-*/out/opencv 2>/dev/null | head -n 1)
grep -n "pub fn new(norm_type" "$OUT"/features2d.rs | head -n 3
grep -n "pub fn create_def()" "$OUT"/features2d.rs | head -n 4
grep -n "pub fn box_points(" "$OUT"/imgproc.rs | head -n 2
grep -n "pub fn min_area_rect(" "$OUT"/imgproc.rs | head -n 2
grep -n "pub fn get_perspective_transform(" "$OUT"/imgproc.rs | head -n 2
grep -n "pub fn warp_perspective(" "$OUT"/imgproc.rs | head -n 2
grep -n "pub fn rotate(" "$OUT"/core.rs | head -n 2
grep -n "pub fn count_non_zero(" "$OUT"/core.rs | head -n 2
grep -n "pub fn mean(" "$OUT"/core.rs | head -n 2
grep -n "pub fn min_max_loc(" "$OUT"/core.rs | head -n 2
