# 架构决策记录（ADR）

每条记录一个决策。状态取值：

- **Proposed**：待确认，当前不实施。
- **Accepted**：已确认，可以实施。
- **Superseded**：被后续决策替代。

| 编号 | 标题 | 状态 | 需要你确认什么 |
|---|---|---|---|
| [0001](0001-vision-core-language.md) | 视觉内核语言：C++ 静态库 + FFI，还是 Rust 绑定 | Proposed | 用 C++ 还是 Rust 写视觉内核 |
| [0002](0002-media-backend.md) | 媒体 I/O：链接 libav，还是调用 FFmpeg 命令行 | Proposed | FFmpeg 的分发方式与许可证 |
| [0003](0003-delivery-phases.md) | 交付分期：文档是终态，需要切成可验收的里程碑 | Proposed | 第一期做哪些功能 |
| [0004](0004-model-licensing.md) | 模型与第三方组件的许可证 | Accepted | 语义对象已删除；剩余模型的许可证在引入时核对 |
| [0005](0005-smart-render-risk.md) | 智能渲染（GOP 拼接）的风险与分期 | Proposed | 是否先做全量重编码 |
| [0006](0006-content-hash-strategy.md) | 视频内容哈希策略 | Proposed | 完整 SHA-1 还是抽样哈希 |
| [0007](0007-performance-targets.md) | 性能指标：文档中的数字是估算 | Proposed | 以什么作为验收标准 |
| [0008](0008-single-session-snapshot.md) | 单会话、快照驱动的 Tauri 外壳 | Accepted | — |
| [0009](0009-batch-queue.md) | 批量队列按独立会话执行，复核导出保持人工 | Accepted | — |
