# VeilFrame 交接文档

更新日期：2026-11-21

本文档面向接手的人。读完后应当能够：知道项目做到哪一步、怎么跑起来、哪些地方是坑、下一步该做什么。

---

## 1. 项目是什么

本地离线的桌面视频打码工具。导入视频，指定目标（文本、图像、固定区域），系统在视频中定位目标出现的位置并打码，人工复核后导出。

- 设计文档：`first_design.md`（第一节是功能，第二节是技术设计）。
- 界面原型（纯静态，已被 React 版取代）：`ui-mockup.html`。
- 架构决策：`docs/adr/`，索引见 `docs/adr/README.md`。

已经明确砍掉的内容：语义对象（人脸、车牌、证件、YOLO）、脱敏报告、音频脱敏（ASR）、审计哈希链、AES 加密。用户确认项目不对外分发、不商用。

---

## 2. 当前状态一览

| 部分 | 状态 | 说明 |
|---|---|---|
| Rust 核心（vf-*） | 完成，测试通过 | `cargo test --workspace` 共 106 个测试通过，1 个 OCR 冒烟测试默认忽略。包含真实扫描 + 导出 + 像素级校验的端到端测试。 |
| OCR（PP-OCRv5） | 已验证 | `vf-ocr/tests/smoke.rs` 在样图上识别出 13 行中文，置信度基本在 0.96 以上。 |
| 图像匹配（SIFT / NCC） | 代码完成，合成视频验证 | 端到端测试用 NCC 在合成视频里定位白色方块并检查打码后的像素。真实台标/水印的阈值仍未校准。 |
| 打码渲染 | 完成 | 马赛克、模糊、色块、羽化，全部用 OpenCV。 |
| 扫描流水线 | 完成并接入界面 | 抽帧 → OCR/模板匹配 → 感知缓存 → 时序融合 → 段；代理分辨率扫描，坐标回映射。 |
| 导出 | 完成并接入界面 | ffmpeg 解码 → 打码 → 编码，全量重编码，音频 copy。 |
| 前端（React + TS + Vite） | 完成并接入真实后端 | 类型检查、单元测试、打包均通过。编辑器、批量队列、导出/新建目标对话框全部调用 Tauri 命令。 |
| Tauri 外壳 | 代码完成，**未在本地编译过** | 见第 8 节第 1 条。首次真实编译由 GitHub Actions 完成。 |
| 端到端流程 | 核心链路已用合成视频跑通 | 打开视频 → 建目标 → 扫描 → 复核 → 导出有测试覆盖；真机界面仍待验证。 |
| CI/CD | 工作流已写好 | `.github/workflows/ci.yml` 跑测试；`.github/workflows/release.yml` 构建三平台安装包。首次运行还需要盯一次。 |

---

## 3. 环境

### 3.1 Windows 主机

- 项目目录：`d:\self\VeilFrame`，Git 远程 `https://github.com/W1412X/VeilFrame.git`。
- 模型目录（仓库外）：`D:\vf-models\ocr`。
- Node：Vite 开发服务器要用到，依赖通过 npmmirror 安装（`app/.npmrc`）。
- Edge：`C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`，可以无头截图做界面检查。

### 3.2 WSL Ubuntu-20.04（主开发环境）

- Rust 1.97.1，在 `~/.cargo/bin`。非登录 shell 的 PATH 里没有，要用 `scripts/wsl/env.sh` 或 `source ~/.cargo/env`。
- gcc 9.4、clang（`/usr/bin/clang`）、cmake 4.4.5 和 ninja（用户级，装在 `~/.local/bin`）。
- libclang：`~/.local/lib/python3.8/site-packages/clang/native`（pip 包 libclang 提供）。
- ffmpeg 4.2.7（apt），够用，没有 NVENC。
- OpenCV 4.10.0 源码编译安装到 `~/opt/opencv`。源码在 `~/src/opencv-4.10.0`，构建目录 `~/build/opencv`。
- ONNX Runtime 1.20.1 动态库：`~/opt/onnxruntime/lib/libonnxruntime.so`。
- Cargo 构建产物目录：`~/vf-target`。
- GPU：RTX 5060 Laptop（驱动 591.86），目前没有用。

### 3.3 WSL Ubuntu-22.04（Tauri 窗口用）

- `libwebkit2gtk-4.1-dev 2.50.4` 已装好，Rust 1.100.0 已装。
- **当前缺 `libdbus-1-dev`**，`cargo check` 会在 `libdbus-sys` 处失败。按用户要求，没有改动系统配置；这一项交给 CI 或由用户自行决定是否安装。
- ffmpeg 需要装（`sudo apt install ffmpeg`）。
- Tauri 窗口不依赖 OpenCV 的系统安装，OpenCV 由 cargo 链接 `~/opt/opencv`（20.04 那套），所以 22.04 只需能访问同一份库。

### 3.4 为什么 Tauri 窗口要放到 22.04

Tauri 2 需要 `webkit2gtk-4.1`，它在 Ubuntu 20.04 的官方源里没有。Debian 和 Ubuntu 新版本的 4.1 包要求 glibc 2.35 或更高，而 20.04 是 glibc 2.31，硬装会破坏系统库。所以窗口只能在 22.04 编译。其余所有开发仍在 20.04。

---

## 4. 仓库结构

```
VeilFrame/
├── Cargo.toml              根 workspace（exclude app/src-tauri）
├── .cargo/config.toml      rsproxy 稀疏索引镜像（CI 中会删除）
├── .github/workflows/
│   ├── ci.yml              Rust 工作区测试 + 前端类型检查/测试/打包
│   └── release.yml         三平台安装包构建与 Release 上传
├── ci/triplets/            vcpkg 静态链接 triplet（Linux / macOS）
├── docs/
│   ├── HANDOVER.md         本文档
│   └── adr/                架构决策记录 0001–0009
├── crates/
│   ├── vf-project/         .maskproj 数据模型、校验、原子保存
│   ├── vf-core/            编辑器：撤销重做、接受/否决、负样本
│   ├── vf-image/           RGB 帧与 OpenCV Mat 互转
│   ├── vf-media/           ffprobe 探测、顺序抽帧流、抽样内容哈希、无窗口子进程
│   ├── vf-vision/          NCC（matchTemplate）、SIFT + RANSAC 单应
│   ├── vf-render/          打码（OpenCV fillPoly、马赛克、模糊、羽化）
│   ├── vf-fusion/          检测段融合（NMS、IoU、k 命中开段、m 丢失补洞、EMA）
│   ├── vf-queue/           批量任务状态机（失败重试、取消恢复、检查点字段未用于续扫）
│   ├── vf-cache/           SQLite 感知缓存（内容哈希 + 引擎版本 + 参数指纹）
│   ├── vf-text/            文字匹配（包含/精确/正则/模糊）、归一化、OCR trait
│   ├── vf-ocr/             PP-OCRv5 检测（DB）+ 识别（CTC），ort + OpenCV
│   │   └── tests/smoke.rs  真实模型冒烟测试（默认忽略）
│   ├── vf-pipeline/        扫描流水线（抽帧 → 检测 → 缓存 → 融合 → 段）
│   ├── vf-session/         会话编排：打开、扫描、编辑、自动保存、导出配置
│   │   └── tests/e2e.rs    合成视频的扫描 + 导出 + 像素检查
│   └── vf-export/          导出（解码 → 打码 → 编码，临时文件原子改名）
├── app/                    React + TypeScript + Vite 前端
│   ├── src/App.tsx         主界面、命令调用、事件订阅、编辑器/队列切换
│   ├── src/components/     Preview、Timeline、Inspector、Sidebar、Queue、Dialogs、Icon
│   ├── src/domain/         types（对齐 Rust JSON）、segments（纯函数，含测试）
│   ├── src/api.ts          Tauri 命令与事件封装
│   └── src-tauri/          Tauri 外壳（独立 workspace）
│       ├── src/lib.rs      命令、事件、资源路径解析、单会话状态
│       ├── src/batch.rs    批量队列管理：添加、取消、恢复、后台工作线程
│       ├── resources/      随包资源占位（bin、onnxruntime、models），CI 填充
│       └── capabilities/   主窗口核心权限 + 文件对话框
├── scripts/
│   ├── wsl/                WSL 脚本（见第 5 节）
│   └── tools/              Python 工具（字典导出、模型转换）
└── models/                 （不入库，模型放在 D:\vf-models）
```

---

## 5. 常用命令

以下命令都在 WSL Ubuntu-20.04 中执行，经由 PowerShell 调用时用 `wsl -d Ubuntu-20.04 -- bash /mnt/d/self/VeilFrame/scripts/wsl/<脚本>`。

| 用途 | 脚本或命令 |
|---|---|
| 加载环境变量（OpenCV、libclang、ORT、PATH） | `source /mnt/d/self/VeilFrame/scripts/wsl/env.sh` |
| 完整工作区编译和测试 | `cargo test --workspace` |
| 只测不依赖 OpenCV 的 crate | `scripts/wsl/test-pure.sh` |
| 汇总测试日志 | `scripts/wsl/summary.sh <log>`，`fails.sh <log>` |
| 看编译错误 | `scripts/wsl/errors.sh <log>` |
| OCR 冒烟测试 | 见下方 |
| 重新编译 OpenCV | `scripts/wsl/build-opencv.sh`（增量） |
| 前端类型检查 | `cd app && npm run typecheck` |
| 前端单元测试 | `cd app && npm test` |
| 前端打包 | `cd app && npm run build` |
| 前端开发服务器 | `cd app && npm run dev`，地址 http://localhost:1420/ |
| Tauri 编译检查（22.04） | `scripts/wsl/tauri-22.sh check`（当前受 dbus 依赖阻塞） |
| 三平台安装包 | push `v*` tag 或手动触发 Release 工作流 |

OCR 冒烟测试：

```bash
export VF_OCR_IMAGE=/mnt/d/android_proj/autop/poc/visual-parsing/samples/sample_settings.png
export VF_OCR_DET=/mnt/d/vf-models/ocr/det_v5.onnx
export VF_OCR_REC=/mnt/d/vf-models/ocr/rec_v5.onnx
export VF_OCR_DICT=/mnt/d/vf-models/ocr/ppocr_keys_v5_onnx.txt
source /mnt/d/self/VeilFrame/scripts/wsl/env.sh
cargo test -p vf-ocr --test smoke -- --ignored --nocapture
```

---

## 6. 模型、依赖与打包资源

### 6.1 模型

| 文件 | 来源 | 备注 |
|---|---|---|
| `det_v5.onnx` | Hugging Face `breezedeus/cnstd-ppocr-ch_PP-OCRv5_det` | PP-OCRv5 mobile 检测，4.8 MB。 |
| `rec_v5.onnx` | Hugging Face `breezedeus/cnocr-ppocr-ch_PP-OCRv5` | PP-OCRv5 mobile 识别，16.6 MB。 |
| `ppocr_keys_v5_onnx.txt` | `scripts/tools/extract_ppocr_v5_dict.py` 从 rec 元数据导出，已入库 | 18383 个字符，与模型匹配，随安装包分发。 |

PP-OCRv5 的 Paddle 权重转 ONNX 失败：PoC 环境中的 `paddle2onnx` 1.3.1 解析不了 v5 的 PIR 格式（`scripts/tools/convert_ocr.py` 会误报成功）。所以改用已经转好的 ONNX。

### 6.2 运行时依赖

| 组件 | 来源 | 放置位置 |
|---|---|---|
| ONNX Runtime 1.20.1 | NuGet `Microsoft.ML.OnnxRuntime`（`scripts/ci/fetch-ort.sh` 解原生库） | `app/src-tauri/resources/onnxruntime/` |
| ffmpeg / ffprobe | npm `ffmpeg-static@5.3.0`、`ffprobe-static@3.1.0`（静态构建） | `app/src-tauri/resources/bin/` |
| PP-OCRv5 模型与字典 | Hugging Face（CI 下载） | `app/src-tauri/resources/models/` |
| OpenCV | vcpkg 静态构建（CI，钉在 vcpkg `9b75e789` = opencv4 4.10.0，与本地 4.10 / CI apt 4.5.4 同代） | 直接链入可执行文件 |

运行时优先级：环境变量 `VF_FFMPEG` / `VF_ORT_LIB` / `ORT_DYLIB_PATH` / `VF_MODELS_DIR` 优先，其次用随包资源，最后回退系统 PATH。`tool_status` 命令会报告四项是否就位，界面顶栏据此提示。

### 6.3 CI/CD

- `ci.yml`：Ubuntu 22.04 上用 apt OpenCV 4.5.4 + ffmpeg 跑 Rust 测试；另起前端任务做类型检查、测试、打包。`ort` 是 `load-dynamic`，测试不加载 ONNX Runtime（OCR 冒烟测试是 `#[ignore]`），所以 CI 不下载它。
- `release.yml`：矩阵构建 Linux x64（deb + AppImage）、Windows x64（NSIS）、macOS arm64（dmg）。
  - OpenCV 用 vcpkg 静态 triplet（Linux/macOS 用 `ci/triplets/` 覆盖，Windows 用 `x64-windows-static-md`），二进制缓存放在 GitHub Actions cache。vcpkg 仓库钉在 `VCPKG_COMMIT`（opencv4 4.10.0 的最后一个 commit）：4.11 起 `cv::cvtColor` 多了 `AlgorithmHint` 参数，`opencv` crate 按头文件生成绑定，版本不钉住 Release 会编不过。
  - ONNX Runtime 用 `scripts/ci/fetch-ort.sh` 从 NuGet 官方包按平台解原生库（GitHub release 上 v1.20.1 的预编译资产不存在）；ffmpeg/ffprobe 用 npm 静态包；模型从 Hugging Face 下载，失败回退 hf-mirror；字典已入库，不需要现场生成。
  - tag 触发时通过 `softprops/action-gh-release` 上传安装包；普通手动触发只上传 artifact。
  - 首次运行会编译 OpenCV，耗时长；之后命中缓存。

---

## 7. 关键决策（详见 ADR）

- **0001 视觉内核用 Rust**：用 `opencv` crate 和 `ort` crate，不写 C++ 静态库。
- **0002 媒体层用 ffmpeg 命令行**：通过 `MediaBackend` 抽象，后期可换 libav。
- **0003 分期**：用户选择一次性实现全部功能，里程碑只作为验收顺序。
- **0004 许可证**：用户确认不对外分发，AGPL 和 GPL 组件在个人使用范围内可用。
- **0005 智能渲染（GOP 拼接）未做**：第一期只做全量重编码。
- **0006 内容哈希用抽样 BLAKE3**：`vf-media::sampled_content_hash`。
- **0007 性能指标还没有基准**：没有实测数字，不要引用文档里的估算值当验收线。
- **0008 单会话、快照驱动**：Tauri 进程内执行扫描/导出，所有命令返回完整快照，前端不持有第二份编辑状态。
- **0009 批量队列**：每任务独立会话、项目物化到视频旁、失败重试 2 次、复核后人工导出。队列在内存中，不做断点续扫和自动导出。

OCR 选 PP-OCRv5（用户确认），不用 v4。

---

## 8. 已知问题与坑

1. **Tauri 外壳没有本地编译过**。WSL 22.04 缺 `libdbus-1-dev`，而用户要求不要改系统配置。`app/src-tauri` 的代码经过人工审查（类型、Tauri API、线程 Send/Sync），但第一次真实编译在 CI。预期可能出现的问题：Tauri 配置字段、命令参数名、资源路径。
2. **Release 工作流首次运行是未知数**。vcpkg 静态 OpenCV 的 triplet、编译时长、链接依赖、静态 ffmpeg 包在 macOS arm64 上的可用性都还没有实跑。第一次失败要按日志修，不要假设缓存已经正确。
3. **macOS 包未签名**。用户第一次打开可能需要右键 → 打开，或在系统设置里允许。
4. **批量队列是内存态**：重启丢失；单工作线程；重新排队整段重扫；不自动导出（ADR-0009）。
5. **OCR 只做了检测加识别**。方向分类（cls）没有接入，竖排文字靠裁剪后旋转处理。识别是逐行推理（batch 1），整帧 1080p 的速度没有测过。
6. **SIFT 和 NCC 的阈值没有在真实视频上校准**。单元测试和端到端测试只用合成图；`threshold`、`LOWE_RATIO`、`RANSAC` 阈值需要用真样本评估（ADR-0007）。
7. **`ort` 锁定在 `=2.0.0-rc.15`**，API 变化较大，升级前要看 `ort` 源码。
8. **OpenCV 版本在 CI 与本地不同**：本地 4.10，CI 测试用 Ubuntu 22.04 的 4.5.4，发布用 vcpkg 的 4.x。如果 CI 测试编译报缺失 API，优先考虑改用 vcpkg 跑测试。
9. **OpenCV 的 `knn_match` 要先 `add` 训练描述子**，`RotatedRect::new` 返回 `Result`，`box_points` 参数是值不是引用。改动时注意。
10. **羽化用的是高斯模糊后的软掩膜**，小区域（如 4×4 像素）中心 alpha 不会到 1，测试已按此写断言。
11. **导出只支持全量重编码**，音频 copy，没有智能渲染。
12. **ffmpeg 没有 NVENC**（本机 Ubuntu 20.04 的 4.2.7）。导出默认 libx264。
13. **浏览器预览里没有后端**。`api.ts` 中的调用会抛出 `NotDesktopError`，界面据此提示。
14. **应用图标**：2026-11-21 选定的正式图标为「播放遮罩 / 纯白 / 深底」，源文件是 `app/src-tauri/icons/icon-source.svg`（1024 透明底 PNG 为 `icon-source.png`）。改图流程：改 SVG → 按 1024×1024 透明底渲染 PNG → `npx tauri icon src-tauri/icons/icon-source.png --fit cover` → 删除生成的 `android/`、`ios/` 目录（本项目不打包移动端）。
15. **Windows 子进程无黑框**：所有 ffmpeg/ffprobe 调用必须走 `vf_media::media_command`，它加了 `CREATE_NO_WINDOW`。新代码不要直接 `Command::new`。

---

## 9. 给接手者的工作约定（用户明确要求过）

- **大文件下载由用户来做**。模型、库、镜像包，先告诉用户要什么、从哪里下，用户确认后再下。用户明确要求我下载的（ONNX Runtime、OCR 模型）除外。
- **不要在命令里用 `sleep`**。用户明确不喜欢。等待后台任务时直接读日志或状态。
- **不要做半成品**。用 OpenCV 和 ONNX Runtime 完整实现，不要用纯 Rust 替代品糊弄。
- **用中文交流**。文档、注释、ADR 都用中文。
- **不要擅自扩大范围**。语义对象、报告、ASR、AES 都已经删除。
- **有偏离设计的地方要写 ADR**，不要悄悄改。
- **不要替用户改系统配置、不要在本机做 GUI 构建**。代码和测试由我们写，三平台安装包由 GitHub Actions 产出。
- **说话直接**。问题直接回答，不要绕。

---

## 10. PowerShell / WSL 调用的坑

- 在 PowerShell 里直接写 `bash -c '...'` 带管道或 `$(...)` 时，引号经常被拆坏。解决办法：把逻辑写成脚本文件，再用 `wsl ... bash /mnt/d/.../script.sh` 调用。
- `> /tmp/xxx.log` 这样的重定向会被 PowerShell 当成 Windows 路径。要放进 `bash -c` 的引号里。
- 后台命令用 `run_in_background=true`，然后读日志文件。日志统一放 `/tmp/vf-*.log`（WSL 内）或 `$env:TEMP`（Windows 内）。
- 有些长命令会被工具层中断（显示 `^C`），这时要重新执行，不要假设它成功了。
- `npm install` 需要在 `app/` 目录下执行；WSL 命令要加 `MSYS2_ARG_CONV_EXCL='*'`，否则参数会被路径转换。

---

## 11. 下一步（按优先级）

1. **盯一次 Release 工作流**：修首次编译/打包错误，确认三平台产物可用。这是唯一还没验证的大块。
2. **真机跑通完整流程**：在 Windows 安装包里打开视频、建目标、扫描、复核、导出，检查预览、进度、取消、批量队列的实际体验。
3. **校准阈值与性能**：准备 5 段左右带标注的代表性视频，记录检出率、误检率、倍速（ADR-0007），再定默认参数。
4. **OS 级体验**：确认 macOS 未签名包的打开方式，必要时加签名/公证；Linux AppImage 在无 OpenCV 系统上的启动测试。
5. **按需再做**：队列持久化、断点续扫、批量自动导出、智能渲染（ADR-0005/0009）。

---

## 12. 还没有决定的问题

- ADR-0006 的哈希算法和抽样方式，用户没有明确确认。
- ADR-0007 的验收指标（漏检率、误检率、速度），还没有定。
- 是否保留浏览器预览模式（目前保留，方便界面调试）。
- 队列是否需要跨重启持久化和断点续扫（目前不做）。
