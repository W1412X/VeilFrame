# 构建与运行环境（source 后使用）
# OpenCV 4.10（由 build-opencv.sh 安装到 ~/opt/opencv）。不使用 pkg-config，直接指定库与头文件位置。
export OPENCV_INCLUDE_PATHS="$HOME/opt/opencv/include/opencv4"
export OPENCV_LINK_PATHS="$HOME/opt/opencv/lib"
export OPENCV_LINK_LIBS="opencv_core,opencv_imgproc,opencv_imgcodecs,opencv_features2d,opencv_calib3d,opencv_flann"
# libclang（供 opencv crate 的 bindgen 使用，由 pip 包 libclang 提供）
export LIBCLANG_PATH="$HOME/.local/lib/python3.8/site-packages/clang/native"
# ONNX Runtime 动态库（ort 以 load-dynamic 方式加载）
export ORT_DYLIB_PATH="${ORT_DYLIB_PATH:-$HOME/opt/onnxruntime/lib/libonnxruntime.so}"
# 运行测试时需要找到 OpenCV 动态库
export LD_LIBRARY_PATH="$HOME/opt/opencv/lib:${LD_LIBRARY_PATH:-}"
# cargo 与 cmake 的用户级路径
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/vf-target"
