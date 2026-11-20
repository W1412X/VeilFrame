# VeilFrame 终态设计 —— 本地视频目标打码工具

**定位**：完全离线的桌面端视频打码工具。导入视频，指定目标（文本 / 图片 / 区域），系统自动定位全部出现位置并打码，人工复核后导出。

**设计公理**：本地处理不出网 · 召回优先人工确认 · 感知与匹配解耦 · 分析与渲染分离 · 全程非破坏性。

---

## 一、产品功能设计

### 1. 目标定义体系

| 目标类型 | 定义方式 | 匹配能力 | 输出形状 |
|---|---|---|---|
| **文本** | 输入文字，支持包含 / 精确 / 正则 / 模糊（编辑距离≤k）/ 全半角与大小写归一化 / 多语言 | 对 OCR 感知缓存做即时查询，增删改文本目标无需重扫 | 文字四边形 |
| **图像** | 上传图片，或**直接在视频帧上框选截取**；同一目标可挂多个模板变体（不同尺度/光照/角度）；支持 alpha 透明掩码 | 特征点 + 单应矩阵，抗缩放、旋转、透视；自适应降级链覆盖无纹理与透明水印 | **透视四边形** |
| **固定区域** | 手绘矩形 / 多边形 / 字幕安全区 | 无需检测，按时间范围生效 | 多边形 |
| **目标组** | 多目标打包为规则集，可导入导出、跨项目复用 | 组内逻辑（任一命中 / 全部命中） | 合并掩膜 |

### 2. 感知与匹配引擎（自适应路由）

系统按目标特征自动选择引擎，用户可手动覆盖为三档：`极速 / 标准 / 精准`。

| 引擎 | 触发条件 | 特性 |
|---|---|---|
| 多尺度 NCC 模板匹配 | 近静态、不透明、尺度变化小 | 最快，固定台标/水印首选 |
| **SIFT + RANSAC 单应** | 有纹理平面目标（默认） | 尺度/旋转不变，抗中等视角变化，输出四边形 |
| AKAZE | 边缘清晰的图形标志 | 非线性尺度空间保边，二进制描述子更快 |
| **SuperPoint + LightGlue** | 大视角 / 光照剧变 / 弱纹理（精准档） | 深度特征，鲁棒性最强，GPU 推理 |
| 形状/边缘匹配（line2Dup）+ alpha 掩码 NCC | 无纹理纯色 logo、透明水印 | 特征点法盲区的专用通道 |

### 3. 时序定位能力
- **检测–跟踪融合**：抽帧重检测 + 中间帧光流/轻量跟踪器补帧；
- **漂移自愈**：前后向光流误差 + NCC 双校验，漂移即触发重检测；
- **场景切重置**：镜头切换强制重置跟踪并重新检测；
- **多实例**：同帧同目标多处出现，NMS 去重后全部保留；
- **时序融合**：k 帧命中开段、m 帧丢失补洞、四角 Kalman 平滑，消除闪烁与抖动；
- **ROI 限定**：限定扫描区域（如画面下方 20%），大幅提速。

### 4. 打码能力
马赛克 / 高斯模糊 / 纯色块 / 贴图覆盖；**多边形掩膜**（透视贴合不漏边）；外扩边距 + 运动自适应外扩（按目标速度动态加边）；边缘羽化；强度与马赛克块尺寸相对区域自适应；每目标独立样式，支持组级继承与覆盖。

### 5. 复核工作台
多轨时间轴（每目标一轨，段按置信度着色）；检测列表视图（时间/目标/引擎/置信度/缩略图，可排序批量处理）；低置信度段 triage 流；接受/否决 + **负样本反馈**（自动收紧该目标局部阈值）；关键帧四边形编辑；从任意帧向前/向后跟踪传播；漏检手动补框。

### 6. 导出与交付
硬件编码（NVENC / QSV / AMF / VideoToolbox，软编兜底）；**智能渲染**（仅重编码含打码的 GOP，其余流拷贝后拼接，未打码区域零画质损失）；音频默认 copy；批量队列与导出预设。

### 7. 项目与安全
`.maskproj` 项目文件自动保存与崩溃恢复；目标库/规则集导入导出；批量任务队列（断点续扫、失败重试、夜间跑批）；**默认全离线、无遥测、模型本地**；原视频永远只读。

---

## 二、技术设计

### 2.1 技术栈与分层（终态）

| 层 | 选型 | 职责 |
|---|---|---|
| UI 壳 | **Tauri 2 + React + TypeScript** | 界面；时间轴/预览用 WebGL2 + Canvas 混合渲染；二进制帧经本地通道推送 |
| 编排核心 | **Rust**（tokio + rayon + rusqlite + serde） | 任务调度、流水线背压、CPU 并行、缓存、项目序列化 |
| 视觉内核 | **C++20 静态库**（OpenCV 4.x + ONNX Runtime C++ API + CUDA/NPP 自研核），FFI 暴露给 Rust | OCR、特征匹配、跟踪、时序融合、打码渲染 |
| 媒体 I/O | **FFmpeg**（libav* 链接 + sidecar 用于智能渲染拼接/封装） | 硬解硬编、封装、流拷贝 |
| 模型 | 全部本地 ONNX：PP-OCRv5 det/rec、SuperPoint/LightGlue；FP16/INT8 量化 | 无网络依赖 |

### 2.2 数据流架构

```
分析路径（代理分辨率，长边 960px）
[FFmpeg 硬解 → GPU 预处理] → 有界帧队列 → [调度器: 场景切/pHash/抽帧/时间窗切分]
   ├─ 检测帧 → [OCR GPU·批处理] ─────────────┐
   │           [特征匹配 线程池 / LightGlue GPU] ─┼→ [关联·NMS·Kalman·段融合] → SQLite 感知缓存
   └─ 中间帧 → [LK/DIS 光流 · 轻量跟踪器 窗内顺序] ┘
渲染路径（全分辨率）
[FFmpeg 硬解] → [多边形打码 GPU/CPU] → [硬编 NVENC/QSV/AMF/VT] → 智能渲染 GOP 拼接 + 音频 copy
```

### 2.3 图像匹配：特征点 + 单应矩阵（核心）

**管线**：模板特征一次性提取并缓存 → 帧特征提取（限 ROI）→ FLANN/BF 匹配 → **Lowe ratio test (0.75)** → **RANSAC 估计单应 H** → 内点数/内点率/重投影误差/面积/凸性校验 → 模板四角经 H 变换得帧内四边形。

```cpp
// C++ / OpenCV 核心匹配（模板特征已缓存）
std::optional<Match> match(const cv::Mat& frameGray) {
    std::vector<cv::KeyPoint> kp; cv::Mat des;
    sift->detectAndCompute(frameGray, roiMask, kp, des);
    if (des.empty()) return std::nullopt;

    std::vector<std::vector<cv::DMatch>> knn;
    matcher->knnMatch(tplDes, des, knn, 2);
    std::vector<cv::DMatch> good;
    for (auto& m : knn)                       // Lowe ratio test
        if (m.size() == 2 && m[0].distance < 0.75f * m[1].distance)
            good.push_back(m[0]);
    if (good.size() < 10) return std::nullopt;

    std::vector<cv::Point2f> src, dst;
    for (auto& m : good) { src.push_back(tplKp[m.queryIdx].pt);
                           dst.push_back(kp[m.trainIdx].pt); }
    cv::Mat inliers;
    cv::Mat H = cv::findHomography(src, dst, cv::RANSAC, 4.0, inliers);
    int n = cv::countNonZero(inliers);
    if (H.empty() || n < 8 || n < 0.3 * good.size()) return std::nullopt;

    std::vector<cv::Point2f> corners = {{0,0},{(float)tplW,0},
                                        {(float)tplW,(float)tplH},{0,(float)tplH}};
    std::vector<cv::Point2f> quad;
    cv::perspectiveTransform(corners, quad, H);
    if (!cv::isContourConvex(quad) || !areaSanity(quad)) return std::nullopt;
    return Match{quad, confidence(n, good.size(), reprojError)};
}
```

**鲁棒性工程**：
- 小 logo / 弱纹理：模板上采样 2–4× + CLAHE；角点密度低于阈值自动路由到形状匹配；
- 透明水印：alpha 掩码 NCC；无纹理纯色：line2Dup 边缘模板；
- 单应仅对**平面目标**成立（logo/水印/屏幕/文档均满足）；非平面或剧烈形变 → LightGlue 通道；
- 多变体模板取最优匹配，置信度 = f(内点数, 内点率, 重投影误差, 时序一致性)；
- 打码用 `fillPoly` 多边形掩膜，透视倾斜不漏边。

### 2.4 文本感知

PP-OCRv5 det+rec（ONNX，FP16/INT8）。检测按抽帧帧跑，识别 crop **攒批推理**（batch 32）。全量"文本+四边形+置信度+时间"写入感知缓存；文本目标匹配 = 缓存上的字符串/正则/模糊查询，**增删改文本目标零重扫成本**。外挂 SRT/ASS 可作先验加速与交叉校验。

### 2.5 检测–跟踪融合与时序后处理

- **时间窗并行**：视频按 2 秒窗切分，窗内顺序跟踪、窗间独立并行——使跟踪阶段也能多核扩展；
- 抽帧间隔 5–15 帧自适应（按运动量）；平面目标用 LK 金字塔光流跟踪四角增量估计 H，非平面用 DIS 稠密流或轻量 Siamese 跟踪器（GPU）；
- 漂移校验：FB error + 跟踪块与模板 NCC，低于阈值即重检测；场景切强制重置；
- 融合：IoU≥0.3 关联 + NMS 去重；k=2~3 命中开段、m=3~5 丢失补洞；四角 Kalman/EMA 平滑；段级置信度聚合，低置信自动进 triage 队列。

### 2.6 并行化与 GPU 加速

| 维度 | 设计 |
|---|---|
| **流水线并行** | 解码/感知/匹配/跟踪/融合/渲染各级以有界队列连接，背压防 OOM；各级时间重叠 |
| **CPU 并行** | Rust rayon 线程池（核数−2）跑 SIFT/AKAZE/形状匹配/LK/时序融合；时间窗切分使跟踪可并行 |
| **GPU 推理** | 单上下文 + **动态批处理**（det batch 8、rec batch 32、LightGlue batch 4）+ 2–3 CUDA stream 重叠 H2D/推理/D2H；GPU 靠喂批而非堆进程 |
| **零拷贝路径** | NVDEC NV12 surface → CUDA/NPP 转 RGB+缩放 → TensorRT/ORT 推理 → 仅坐标回 CPU；渲染阶段 GPU 打码核 → NVENC |
| **执行后端矩阵** | NVIDIA→TensorRT/CUDA；Windows AMD/Intel→DirectML；macOS→CoreML；Intel CPU→OpenVINO；兜底 CPU+SIMD |
| **硬件编解码** | 解码 NVDEC/QSV/VideoToolbox；编码 NVENC/QSV/AMF/VideoToolbox；音频 `-c copy` |
| **跳过策略** | pHash 静态帧复用、ROI 限定、感知缓存命中跳过、字幕区先验 |

### 2.7 渲染与智能渲染

多边形掩膜 + 外扩 + 运动自适应加边 + 可选羽化；马赛克 = 区域降采样后最近邻放大；高斯 = 可分离核或三次 box blur 近似；GPU 用 NPP/自研 CUDA 核，CPU 用 SIMD。
**智能渲染**：按 GOP 对齐切分，含打码的 GOP 重编码，其余 `-c copy`，concat 合并——未打码区域零损失，长视频导出提速数倍。

### 2.8 感知缓存与数据模型

缓存键 = `视频内容哈希 + 引擎版本 + 参数指纹`，保证可复现与正确失效。

```
SQLite: videos(hash,meta) · scenes(hash,t) · phash(hash,frame,hash)
        ocr(hash,frame,text,quad,conf) · keypoints(hash,frame,blob)
        detections(hash,target,frame,quad,conf,engine) · segments(project,…)
```

```jsonc
// .maskproj
{
  "video": {"path":"…","sha1":"…","fps":30,"w":1920,"h":1080},
  "targets": [{
    "id":"t2","type":"image","color":"#30A46C",
    "variants":["assets/t2_a.png","assets/t2_b.png"],
    "engine":"auto","mode":"standard","threshold":0.6,
    "roi":[0,0,1920,400],
    "style":{"kind":"mosaic","strength":0.7,"padding":8,"feather":2,"motionPad":true}
  }],
  "segments": [{
    "target":"t2","t0":12.3,"t1":18.7,"status":"confirmed","source":"auto",
    "keyframes":[{"t":12.3,"quad":[[x,y],[x,y],[x,y],[x,y]],"conf":0.91}],
    "engine":"sift","negatives":[…]
  }],
  "render":{"codec":"h264_nvenc","smart":true,"audio":"copy"},
  "audioRedaction":{"enabled":true,"words":["…"],"effect":"beep"},
  "report":{"format":"pdf","audit":true}
}
```

### 2.9 性能预算（量级估算，以基准集实测为准）

10 分钟 1080p30 ≈ 18000 帧，RTX 3060 级，代理 960px，抽帧间隔 6：

| 环节 | 耗时 |
|---|---|
| 硬解 + GPU 预处理 | 30–60s |
| OCR 3000 帧（批处理） | 25–40s |
| SIFT 3000 帧（8 线程） | 10–18s |
| 光流补帧 18000 帧（窗并行） | 8–15s |
| 时序融合 | <5s |
| 渲染 + NVENC | 60–100s |
| **标准模式合计** | **≈2.5–4 分钟（0.3× 视频时长）** |
| 精准模式（LightGlue） | ≈4–7 分钟 |
| 纯 CPU | ≈8–20 分钟 |

### 2.10 资源治理与可靠性
代理帧队列 64 帧、全分辨率渲染队列 16 帧；特征点 Top-K 压缩存储 + LRU；长视频按 10 分钟分片；显存预算 ≤ 可用 70%（模型+批+surface）；断点续扫按帧区间 checkpoint；取消即停；导出原子改名；崩溃后项目与缓存双恢复。

### 2.11 安全与质量保障
默认全离线、无遥测、模型本地。
基准视频集覆盖：固定台标、透明水印、滚动字幕、手持拍摄、低光、运动模糊、多实例、大透视、无纹理 logo。指标：人工确认后漏检为 0、段级 IoU、精确率、跨 GPU/CPU 结果一致性、性能回归。

---

以上为 VeilFrame 的完整终态设计：功能与技术两部分，所有能力均为产品标配，性能路径以流水线并行 + 时间窗并行 + GPU 批处理与零拷贝 + 感知缓存 + 智能渲染共同保障。