# ADR-0001：视觉内核语言

- 状态：Accepted（2026-11-20，用户确认：用 Rust。采用方案 B，视觉算子通过 `opencv`、`ort` crate 调用。自研 CUDA 核出现时再评估 C++ 模块）
- 日期：2026-11-20
- 关联：`first_design.md` 4.1 技术栈表（原文为"视觉内核：C++20 静态库，FFI 暴露给 Rust"）

## 背景

文档把技术栈分成四层：React/TS（界面）、Rust（编排）、C++20（视觉内核）、FFmpeg（媒体）。这样会同时需要 MSVC、CMake、OpenCV 与 ONNX Runtime 的 C++ 构建，以及 Rust 与 C++ 之间的 FFI 绑定代码。

当前机器没有 MSVC、CMake、OpenCV、ONNX Runtime 和 Rust 工具链。

## 备选方案

| 方案 | 做法 | 优点 | 缺点 |
|---|---|---|---|
| A. 按文档 | C++20 静态库，cxx 或手写 FFI 绑定到 Rust | 与文档一致；后期 CUDA/NPP 自研核有 C++ 生态 | 四种语言；构建链路长；FFI 边界要维护 |
| B. Rust 绑定 | 用 `opencv` crate 与 `ort` crate 直接在 Rust 中调用 OpenCV 与 ONNX Runtime | 单一后端语言；无 FFI 层；构建只需配置原生库路径 | 绑定覆盖不全时需自己补；大规模 CUDA 自研核仍需 C++ 或 cudarc 之类的库 |
| C. 混合 | 先用 B 开发，只有确需自研 CUDA 核时才引入 C++ 模块 | 先降低风险，保留后路 | 边界要在第一期就设计清楚 |

## 我的建议

选 **C**。第一期（ADR-0003 中的 M0–M2）全部在 Rust 里完成，视觉算子封装在 `vision` crate 的 trait 后面。等真正需要自研 CUDA 核（文档 4.6 的零拷贝打码核、4.7 的 GPU 渲染）时，再以独立 C++ 库的形式接入，只改 `vision` crate 的实现，不改上层接口。

## 需要你确认

1. 第一期是否接受 A 改为 B/C（偏离文档 4.1 的"C++20 静态库"）。
2. 如果坚持 A，我需要先安装 Visual Studio Build Tools（C++ 工作负载）、CMake、OpenCV 4.x 与 ONNX Runtime 的预编译包。这些都是系统级安装，我不会在未经你同意时执行。
