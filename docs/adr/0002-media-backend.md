# ADR-0002：媒体 I/O 后端

- 状态：Accepted（2026-11-20，采用方案 C：`MediaBackend` trait，第一期用 ffprobe/ffmpeg 命令行。用户确认不对外分发，GPL 构建可用于本地）
- 日期：2026-11-20
- 关联：`first_design.md` 4.1（"FFmpeg：libav* 链接 + sidecar 用于智能渲染拼接/封装"）

## 背景

文档要求同时用 libav* 链接和 sidecar 命令行两种方式。链接 libav 意味着要自己编译 FFmpeg，还要处理 NVDEC/NVENC 的编译选项。FFmpeg 的许可证取决于编译时启用的组件：启用 `--enable-gpl`（如 x264、x265）后整个二进制是 GPL；否则是 LGPL。

本机当前没有 `ffmpeg` 与 `ffprobe`。

## 备选方案

| 方案 | 做法 | 优点 | 缺点 |
|---|---|---|---|
| A. 全部链接 libav | 用 `ffmpeg-next` 等绑定，直接读帧、写帧 | 无进程开销；能做零拷贝 | 构建复杂；许可证随编译选项变化；大量 unsafe 代码 |
| B. 全部 sidecar | 调用 `ffprobe`、`ffmpeg` 命令行，解析输出 | 实现简单；许可证边界清楚（进程隔离）；易替换 | 帧数据要经管道传输，开销大；无法精细控制 GOP |
| C. 分层 | 定义 `MediaBackend` trait。MVP 用 B，后期性能瓶颈处替换为 A | 与 ADR-0001 的分层一致 | 需要在第一期定好 trait |

## 我的建议

选 **C，MVP 先用 B**。进程隔离可以把 FFmpeg 的许可证风险限制在分发包层面。需要的话，我会在 `media` crate 中定义 `MediaBackend` trait（探测元数据、按时间戳抽帧、按 GOP 拼接、编码输出）。

## 需要你确认

1. 分发时是否愿意随包附带一个 LGPL 构建的 `ffmpeg.exe`/`ffprobe.exe`（不含 x264/x265），还是由用户自行安装。
2. 如果需要 H.264 软编（文档 6 节"软编兜底"），是否接受 GPL 构建，或改用其他许可证的编码器。
3. NVENC 硬编依赖 NVIDIA 驱动与对应版本的 FFmpeg 构建，这一点需要在安装说明中写明。
