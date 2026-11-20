# VeilFrame 交接文档

更新日期：2026-11-20

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
| Rust 核心（vf-*） | 可编译，测试通过 | 上一次完整工作区运行全部通过（74 个）。之后 `vf-media` 增加了 4 个内容哈希测试，纯 Rust crate 已单独重跑通过，完整工作区还没有重跑。 |
| OCR（PP-OCRv5） | 已验证 | 用 `vf-ocr/tests/smoke.rs` 在 PoC 的样图 `sample_settings.png` 上识别出 13 行中文，置信度基本在 0.96 以上。 |
| 图像匹配（SIFT / NCC） | 代码写完，未在真实视频上验证 | 用 OpenCV 实现，单元测试只用合成图。 |
| 打码渲染 | 代码写完，有单元测试 | 马赛克、模糊、色块、羽化，全部用 OpenCV。 |
| 扫描流水线 | 代码写完，有单元测试 | `vf-pipeline`，用假 OCR 测了流程。没有接到界面上。 |
| 导出 | 代码写完，有单元测试 | `vf-export`，ffmpeg 解码 → 打码 → 编码。没有用真视频跑过。 |
| 前端（React + TS + Vite） | 可运行 | 类型检查和打包通过。界面用示例数据，可以交互（选中、接受/否决、拖动四角、撤销、批量队列页、导出和新建目标对话框）。 |
| Tauri 外壳 | 代码写完，未编译 | `app/src-tauri/`，需要 Ubuntu-22.04 的 Rust。 |
| 端到端流程 | 未跑通 | 打开视频 → 扫描 → 复核 → 导出，这条链路还没有连起来。 |

---

## 3. 环境

### 3.1 Windows 主机

- 项目目录：`d:\self\VeilFrame`
- 模型目录（仓库外）：`D:\vf-models\ocr`
- Node：Vite 开发服务器要用到，依赖通过 npmmirror 安装（`app/.npmrc`）。
- Edge：`C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`，可以无头截图做界面检查。

### 3.2 WSL Ubuntu-20.04（主开发环境）

- Rust 1.97.1，在 `~/.cargo/bin`。非登录 shell 的 PATH 里没有，要用 `scripts/wsl/env.sh` 或 `source ~/.cargo/env`。
- gcc 9.4、clang（`/usr/bin/clang`）、cmake 4.4.5 和 ninja（用户级，装在 `~/.local/bin`）。
- libclang：`~/.local/lib/python3.8/site-packages/clang/native`（pip 包 libclang 提供）。
- ffmpeg 4.2.7（apt），够用，没有 NVENC。
- OpenCV 4.10.0 源码编译安装到 `~/opt/opencv`。源码在 `~/src/opencv-4.10.0`，构建目录 `~/build/opencv`。
- ONNX Runtime 1.20.1 动态库：`~/opt/onnxruntime/lib/libonnxruntime.so`（从清华镜像的 cp310 wheel 解出来的）。
- Cargo 构建产物目录：`~/vf-target`。
- GPU：RTX 5060 Laptop（驱动 591.86），目前没有用。

### 3.3 WSL Ubuntu-22.04（只用于 Tauri 窗口）

- `libwebkit2gtk-4.1-dev 2.50.4` 已经装好。
- Rust 需要用户自己装（命令见 `docs/` 或对话记录里的 rsproxy 方案）。
- ffmpeg 需要装（`sudo apt install ffmpeg`）。
- Tauri 窗口不依赖 OpenCV，所以 22.04 不需要装 OpenCV。

### 3.4 为什么 Tauri 窗口要放到 22.04

Tauri 2 需要 `webkit2gtk-4.1`，它在 Ubuntu 20.04 的官方源里没有。Debian 和 Ubuntu 新版本的 4.1 包要求 glibc 2.35 或更高，而 20.04 是 glibc 2.31，硬装会破坏系统库。所以窗口只能在 22.04 编译。其余所有开发仍在 20.04。

---

## 4. 仓库结构

```
VeilFrame/
├── Cargo.toml              根 workspace（exclude app/src-tauri）
├── .cargo/config.toml      rsproxy 稀疏索引镜像（项目级，不改全局）
├── first_design.md         设计文档
├── ui-mockup.html          早期静态原型（参考用）
├── docs/
│   ├── HANDOVER.md         本文档
│   └── adr/                架构决策记录 0001–0007
├── crates/
│   ├── vf-project/         .maskproj 数据模型、校验、原子保存
│   ├── vf-core/            编辑器：撤销重做、接受/否决、负样本
│   ├── vf-image/           RGB 帧与 OpenCV Mat 互转
│   ├── vf-media/           ffprobe 探测、顺序抽帧流、抽样内容哈希
│   ├── vf-vision/          NCC（matchTemplate）、SIFT + RANSAC 单应
│   ├── vf-render/          打码（OpenCV fillPoly、马赛克、模糊、羽化）
│   ├── vf-fusion/          检测段融合（NMS、IoU、k 命中开段、m 丢失补洞、EMA）
│   ├── vf-queue/           批量任务状态机，支持检查点续扫
│   ├── vf-cache/           SQLite 感知缓存（内容哈希 + 引擎版本 + 参数指纹）
│   ├── vf-text/            文字匹配（包含/精确/正则/模糊）、归一化、OCR trait
│   ├── vf-ocr/             PP-OCRv5 检测（DB）+ 识别（CTC），ort + OpenCV
│   │   └── tests/smoke.rs  真实模型冒烟测试（默认忽略）
│   ├── vf-pipeline/        扫描流水线（抽帧 → 检测 → 缓存 → 融合 → 段）
│   └── vf-export/          导出（解码 → 打码 → 编码，临时文件原子改名）
├── app/                    React + TypeScript + Vite 前端
│   ├── src/App.tsx         主界面
│   ├── src/components/     Preview、Timeline、Inspector、Sidebar、Queue、Dialogs、Icon
│   ├── src/domain/         types（对齐 Rust JSON）、editor（撤销重做）、sample（示例数据）
│   ├── src/api.ts          Tauri 命令封装（浏览器中调用会提示"需要桌面应用"）
│   └── src-tauri/          Tauri 外壳（独立 workspace，未编译）
│       ├── src/lib.rs      命令：tool_status、new_project、load_project、save_project
│       ├── tauri.conf.json devUrl = http://localhost:1420
│       └── capabilities/   dialog 与 core 权限
├── scripts/
│   ├── wsl/                WSL 脚本（见第 5 节）
│   └── tools/              Python 工具（字典导出、转换）
└── models/                 （不入库，模型放在 D:\vf-models）
```

---

## 5. 常用命令

以下命令都在 WSL Ubuntu-20.04 中执行，经由 PowerShell 调用时用 `wsl -d Ubuntu-20.04 -- bash /mnt/d/self/VeilFrame/scripts/wsl/<脚本>`。

| 用途 | 脚本或命令 |
|---|---|
| 加载环境变量（OpenCV、libclang、ORT、PATH） | `source /mnt/d/self/VeilFrame/scripts/wsl/env.sh` |
| 完整工作区编译和测试 | `scripts/wsl/test.sh` |
| 只测不依赖 OpenCV 的 crate | `scripts/wsl/test-pure.sh` |
| 汇总测试日志 | `scripts/wsl/summary.sh <log>`，`fails.sh <log>` |
| 看编译错误 | `scripts/wsl/errors.sh <log>` |
| 查 crate 的 API 签名（从生成代码中） | `scripts/wsl/api-sigs*.sh` |
| OCR 冒烟测试 | 见下方 |
| 重新编译 OpenCV | `scripts/wsl/build-opencv.sh`（增量） |
| 前端类型检查 | `cd app && npm run typecheck` |
| 前端打包 | `cd app && npm run build` |
| 前端开发服务器 | `cd app && npm run dev`，地址 http://localhost:1420/ （绑定 127.0.0.1） |
| Tauri 编译检查（22.04） | `scripts/wsl/tauri-22.sh check` |
| Tauri 运行（22.04） | 先在 Windows 启动前端，再 `scripts/wsl/tauri-22.sh run` |

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

## 6. 模型与依赖的来源

| 文件 | 来源 | 备注 |
|---|---|---|
| `D:\vf-models\ocr\det_v5.onnx` | hf-mirror `breezedeus/cnstd-ppocr-ch_PP-OCRv5_det` | PP-OCRv5 mobile 检测，4.8 MB。当前使用。 |
| `D:\vf-models\ocr\rec_v5.onnx` | hf-mirror `breezedeus/cnocr-ppocr-ch_PP-OCRv5` | PP-OCRv5 mobile 识别，16.6 MB。当前使用。 |
| `D:\vf-models\ocr\ppocr_keys_v5_onnx.txt` | 从 rec_v5.onnx 的 metadata 导出（18383 个字符） | 与模型匹配，用这个。 |
| `D:\vf-models\ocr\ppocr_keys_v5.txt` | 从 Paddle 的 inference.yml 导出 | 备用，条数一致。 |
| `D:\vf-models\ocr\det_v4.onnx`、`rec_v4.onnx`、`ppocr_keys_v4.txt` | PoC 的 rapidocr 自带 | v4 备用，当前不用。 |
| `~/opt/onnxruntime/lib/libonnxruntime.so` | 清华 PyPI 镜像，onnxruntime 1.20.1 cp310 wheel | `ORT_DYLIB_PATH` 指向这里。 |
| `~/opt/opencv` | 源码 `opencv-4.10.0.zip`（在 `~/vf-deps`） | 只编译了 core、imgproc、imgcodecs、features2d、calib3d、flann。 |

PP-OCRv5 的 Paddle 权重转 ONNX 失败：PoC 环境中的 `paddle2onnx` 1.3.1 解析不了 v5 的 PIR 格式（`scripts/tools/convert_ocr.py` 里脚本会误报成功，要注意）。所以改用了 hf-mirror 上已经转好的 ONNX。

PoC 目录 `D:\android_proj\autop\poc\visual-parsing` 是另一个项目，只借用了它的样图和 rapidocr 字典，没有依赖它的代码。

---

## 7. 关键决策（详见 ADR）

- **0001 视觉内核用 Rust**：用 `opencv` crate 和 `ort` crate，不写 C++ 静态库。
- **0002 媒体层用 ffmpeg 命令行**：通过 `MediaBackend` 抽象，后期可换 libav。
- **0003 分期**：用户选择一次性实现全部功能，里程碑只作为验收顺序。
- **0004 许可证**：用户确认不对外分发，AGPL 和 GPL 组件在个人使用范围内可用。
- **0005 智能渲染（GOP 拼接）未做**：第一期只做全量重编码。
- **0006 内容哈希用抽样 BLAKE3**：用户没有明确确认，但我按建议实现了。`vf-media::sampled_content_hash`。
- **0007 性能指标还没有基准**：没有实测数字，不要引用文档里的估算值当验收线。

OCR 选 PP-OCRv5（用户确认），不用 v4。

---

## 8. 已知问题与坑

1. **Tauri 外壳还没编译过**。第一次编译大概率会报错（API、配置或图标），需要在 22.04 上修。
2. **扫描和导出没有接到界面**。前端的"导出"和"扫描"按钮目前只是记录参数并弹出提示。
3. **前后端有两份编辑逻辑**。Rust 的 `vf-core::Editor` 和前端的 `app/src/domain/editor.ts` 行为一致，但是是两份代码。接入后端后应该只保留一份，建议以 Rust 为准。
4. **OCR 只做了检测加识别**。方向分类（cls）没有接入，竖排文字靠裁剪后旋转处理。识别是逐行推理（batch 1），整帧 1080p 的速度没有测过。
5. **SIFT 和 NCC 没有在真实视频上测过**。单元测试只用合成图，阈值（比如 `threshold`、`LOWE_RATIO`、`RANSAC` 阈值）都是初始值，需要用真样本校准（ADR-0007）。
6. **`ort` 的版本锁定在 `=2.0.0-rc.15`**，API 变化较大，升级前要看 `ort` 的源码。
7. **OpenCV 的 `knn_match` 要先 `add` 训练描述子**，`RotatedRect::new` 返回 `Result`，`box_points` 的参数是值不是引用。这些在 `vf-vision/src/sift.rs` 和 `vf-ocr/src/lib.rs` 里已经处理，改动时注意。
8. **羽化用的是高斯模糊后的软掩膜**，小区域（比如 4×4 像素）的中心 alpha 不会到 1，测试里已经按这一点写了断言。
9. **导出只支持全量重编码**，音频是 `copy`，没有智能渲染。
10. **`scripts/tools/convert_ocr.py`** 的错误处理有问题：paddle2onnx 失败时退出码仍是 0，会打印 `ok`。不要相信它的输出，要检查文件大小。
11. **ffmpeg 没有 NVENC**（Ubuntu 20.04 的 4.2.7 版本）。导出默认用 x264。
12. **浏览器预览里没有后端**。`api.ts` 中的调用会抛出 `NotDesktopError`，界面据此提示。

---

## 9. 给接手者的工作约定（用户明确要求过）

- **大文件下载由用户来做**。模型、库、镜像包，先告诉用户要什么、从哪里下，用户确认后再下。用户明确要求我下载的（ONNX Runtime、OCR 模型）除外。
- **不要在命令里用 `sleep`**。用户明确不喜欢。等待后台任务时直接读日志或状态。
- **不要做半成品**。用户明确要求用 OpenCV 和 ONNX Runtime 完整实现，不要用纯 Rust 替代品糊弄。
- **用中文交流**。文档、注释、ADR 都用中文。
- **不要擅自扩大范围**。语义对象、报告、ASR、AES 都已经删除。
- **有偏离设计的地方要写 ADR**，不要悄悄改。
- **说话直接**。问题直接回答，不要绕。

---

## 10. PowerShell 调用的坑（本机环境特有）

- 在 PowerShell 里直接写 `bash -c '...'` 带管道或 `$(...)` 时，引号经常被拆坏。解决办法：把逻辑写成脚本文件，再用 `wsl ... bash /mnt/d/.../script.sh` 调用。
- `> /tmp/xxx.log` 这样的重定向会被 PowerShell 当成 Windows 路径。要放进 `bash -c` 的引号里。
- 后台命令用 `run_in_background=true`，然后读日志文件。日志统一放 `/tmp/vf-*.log`（WSL 内）或 `$env:TEMP`（Windows 内）。
- 有些长命令会被工具层中断（显示 `^C`），这时要重新执行，不要假设它成功了。
- `npm install` 需要在 `app/` 目录下执行。

---

## 11. 下一步（按优先级）

1. **在 22.04 编译 Tauri 外壳**：用户装好 Rust 后执行 `scripts/wsl/tauri-22.sh check`，修报错。
2. **打开视频、保存项目跑通**：用真视频测 `new_project`（ffprobe + 抽样哈希）和 `save_project`。
3. **决定扫描放在哪边跑**。选项：
   - (a) 在 22.04 也装 OpenCV 4.10 和 ORT，Tauri 命令直接调用 `vf-pipeline`。
   - (b) 扫描做成独立的 CLI，由 Tauri 通过子进程调用。
   - 建议 (b)，窗口进程保持轻量，OpenCV 的依赖只在 20.04 上管理。
4. **生成一段带文字的测试视频**：用 ffmpeg 或 OpenCV 画字，用来端到端测试扫描、融合、导出。
5. **接入扫描和导出**：前端的扫描按钮、进度条、复核结果显示。
6. **统一编辑逻辑**：决定以 Rust 为准还是以前端为准（见第 8 节第 3 条）。
7. **校准阈值**：用真样本评估 SIFT、NCC、OCR 的检出率和误检率，按 ADR-0007 记录。
8. **智能渲染**：等全量导出稳定后再做（ADR-0005）。

---

## 12. 还没有决定的问题

- ADR-0006 的哈希算法和抽样方式，用户没有明确确认。
- ADR-0007 的验收指标（漏检率、误检率、速度），还没有定。
- 扫描放在哪边跑（见第 11 节第 3 条）。
- 是否保留浏览器预览模式（目前保留，方便界面调试）。
