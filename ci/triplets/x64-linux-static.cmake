# Linux 静态库（Rust 默认动态 CRT），让 OpenCV 打进可执行文件，
# AppImage 不依赖用户系统里的 OpenCV。
set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE static)
set(VCPKG_CMAKE_SYSTEM_NAME Linux)
