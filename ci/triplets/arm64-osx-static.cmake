# macOS arm64 静态库，避免 .app 依赖 Homebrew 的 dylib。
set(VCPKG_TARGET_ARCHITECTURE arm64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE static)
# 没有这行 vcpkg 会把它当 Windows/MSVC triplet 直接报错。
set(VCPKG_CMAKE_SYSTEM_NAME Darwin)
set(VCPKG_OSX_ARCHITECTURES arm64)
set(VCPKG_OSX_DEPLOYMENT_TARGET 11.0)
