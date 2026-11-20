#!/usr/bin/env bash
# 从源码构建 OpenCV 4.10（只编译本项目需要的模块），安装到 ~/opt/opencv
# 模块：core / imgproc / imgcodecs / features2d（SIFT、AKAZE）/ calib3d（RANSAC 单应）/ flann
set -euo pipefail
export PATH="$HOME/.local/bin:$PATH"
SRC="$HOME/src/opencv-4.10.0"
BUILD="$HOME/build/opencv"
PREFIX="$HOME/opt/opencv"

if [ ! -d "$SRC" ]; then
  mkdir -p "$HOME/src"
  unzip -q "$HOME/vf-deps/opencv-4.10.0.zip" -d "$HOME/src"
fi

cmake -G Ninja -S "$SRC" -B "$BUILD" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DBUILD_LIST=core,imgproc,imgcodecs,features2d,calib3d,flann \
  -DBUILD_TESTS=OFF -DBUILD_PERF_TESTS=OFF -DBUILD_EXAMPLES=OFF -DBUILD_DOCS=OFF \
  -DBUILD_opencv_python3=OFF -DBUILD_JAVA=OFF -DBUILD_opencv_apps=OFF \
  -DWITH_GTK=OFF -DWITH_QT=OFF -DWITH_FFMPEG=OFF -DWITH_GSTREAMER=OFF \
  -DWITH_IPP=OFF -DWITH_ITT=OFF -DWITH_OPENCL=OFF -DWITH_OPENEXR=OFF \
  -DWITH_CUDA=OFF -DWITH_TBB=OFF \
  -DOPENCV_GENERATE_PKGCONFIG=OFF \
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5

cmake --build "$BUILD" -j"$(nproc)"
cmake --install "$BUILD"
echo "OpenCV installed to $PREFIX"
