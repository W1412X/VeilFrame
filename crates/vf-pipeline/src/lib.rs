//! 扫描流水线（对应 `first_design.md` 2.2、2.5、2.8）。
//!
//! 流程：按采样间隔顺序抽帧 → 检测（先查感知缓存）→ 时序融合为检测段。
//! 取消通过原子标志实现，取消后返回已扫描部分的结果与检查点（续扫用）。
//!
//! 检测器：
//! - `TextDetector`：OCR（PP-OCRv5）+ 文字匹配（`vf-text`）。
//! - `SiftDetector`：SIFT + RANSAC 单应（标准档）。
//! - `NccDetector`：`matchTemplate` 多尺度 NCC（极速档）。
//!
//! 组合器：`RoiDetector` 把检测限定在区域并在坐标上还原，`ThresholdDetector` 按置信度过滤，
//! `MultiDetector` 汇总多个模板变体的结果。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::thread;

use opencv::core;
use vf_cache::{Namespace, PerceptualCache};
use vf_fusion::{fuse, Detection, FusedSegment, FusionParams};
use vf_image::{crop_rgb, gray_from_rgb, RgbFrame};
use vf_media::stream::FrameStream;
use vf_project::{Engine, Keyframe, Segment, SegmentStatus, Source};
use vf_text::{sub_quad, OcrEngine, OcrLine, TextQuery};
use vf_vision::ncc::NccTemplate;
use vf_vision::sift::SiftMatcher;

/// 低于此置信度的段标记为"低置信"，进入 triage 队列。
pub const LOW_CONFIDENCE: f64 = 0.6;

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("抽帧失败：{0}")]
    Source(String),
    #[error("检测失败：{0}")]
    Detector(String),
    #[error("缓存失败：{0}")]
    Cache(String),
}

/// 按采样顺序逐帧输出 RGB 帧，到达末尾返回 `Ok(None)`。
pub trait FrameSource {
    fn next_frame(&mut self) -> Result<Option<RgbFrame>, String>;
}

/// 单帧检测器。
pub trait Detector {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String>;
}

impl<T: Detector + ?Sized> Detector for Box<T> {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        (**self).detect(frame)
    }
}

pub struct TextDetector<O: OcrEngine> {
    pub ocr: O,
    pub query: TextQuery,
}

impl<O: OcrEngine> Detector for TextDetector<O> {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        let lines = self.ocr.recognize(frame.width, frame.height, &frame.data)?;
        let mut out = Vec::new();
        for line in lines {
            if let Some(hit) = self.query.find(&line.text) {
                let n = line.text.chars().count();
                out.push(Detection { quad: sub_quad(&line.quad, n, hit), conf: line.conf });
            }
        }
        Ok(out)
    }
}

pub struct SiftDetector {
    pub matcher: SiftMatcher,
}

impl Detector for SiftDetector {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        let gray = gray_from_rgb(frame.width, frame.height, &frame.data).map_err(|e| e.to_string())?;
        match self.matcher.locate(&gray).map_err(|e| e.to_string())? {
            Some(loc) => Ok(vec![Detection { quad: loc.quad, conf: loc.conf }]),
            None => Ok(Vec::new()),
        }
    }
}

pub struct NccDetector {
    pub template: NccTemplate,
}

impl Detector for NccDetector {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        let gray = gray_from_rgb(frame.width, frame.height, &frame.data).map_err(|e| e.to_string())?;
        let roi = core::Rect::new(0, 0, frame.width as i32, frame.height as i32);
        let matches = self.template.find(&gray, roi).map_err(|e| e.to_string())?;
        Ok(matches.iter().map(|m| Detection { quad: m.quad(), conf: m.score }).collect())
    }
}

/// 把内层检测器限制在帧内区域：先裁剪再检测，最后把四边形平移回整帧坐标。
/// 区域超出帧范围的部分会被截断；完全落在帧外时报错。
pub struct RoiDetector<D> {
    pub inner: D,
    /// `(x, y, w, h)`，帧像素坐标。
    pub roi: (u32, u32, u32, u32),
}

impl<D: Detector> Detector for RoiDetector<D> {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        let (x, y, w, h) = self.roi;
        let cropped = crop_rgb(frame, x, y, w, h).ok_or_else(|| {
            format!("ROI ({x},{y},{w},{h}) 超出帧范围 {}x{}", frame.width, frame.height)
        })?;
        let mut dets = self.inner.detect(&cropped)?;
        for d in dets.iter_mut() {
            for p in d.quad.iter_mut() {
                p[0] += x as f64;
                p[1] += y as f64;
            }
        }
        Ok(dets)
    }
}

/// 丢弃置信度低于阈值的检测。
pub struct ThresholdDetector<D> {
    pub inner: D,
    pub min_conf: f64,
}

impl<D: Detector> Detector for ThresholdDetector<D> {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        let mut dets = self.inner.detect(frame)?;
        dets.retain(|d| d.conf >= self.min_conf);
        Ok(dets)
    }
}

/// 依次运行多个检测器（如多套模板变体），汇总全部命中；重叠由融合阶段的 NMS 去重。
pub struct MultiDetector {
    pub detectors: Vec<Box<dyn Detector>>,
}

impl Detector for MultiDetector {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        let mut out = Vec::new();
        for d in self.detectors.iter_mut() {
            out.extend(d.detect(frame)?);
        }
        Ok(out)
    }
}

/// 通过 ffmpeg 顺序解码的帧源。`fps` 为采样频率，`size` 为代理分辨率。
pub struct FfmpegFrameSource {
    stream: FrameStream,
}

impl FfmpegFrameSource {
    /// `range` 为 `(起点秒, 时长秒)`，`None` 表示整段视频。起点请用 [`sample_window`] 对齐。
    pub fn open(
        ffmpeg: &Path,
        input: &Path,
        fps: f64,
        size: Option<(usize, usize)>,
        range: Option<(f64, f64)>,
    ) -> Result<Self, String> {
        let stream = FrameStream::open_range(ffmpeg, input, Some(fps), size, range).map_err(|e| e.to_string())?;
        Ok(Self { stream })
    }
}

impl FrameSource for FfmpegFrameSource {
    fn next_frame(&mut self) -> Result<Option<RgbFrame>, String> {
        match self.stream.next_frame().map_err(|e| e.to_string())? {
            Some(data) => Ok(Some(RgbFrame {
                width: self.stream.width(),
                height: self.stream.height(),
                data,
            })),
            None => Ok(None),
        }
    }
}

/// 扫描参数。时间均为视频内的绝对时间（秒）。
pub struct ScanConfig<'a> {
    /// 扫描起点（秒）。会对齐到采样网格。
    pub start: f64,
    /// 扫描终点（秒，包含）。
    pub end: f64,
    /// 采样间隔（秒）。`FrameSource` 必须从 [`sample_window`] 给出的起点开始，按同样的间隔输出帧。
    pub interval: f64,
    /// 检测线程数（至少为 1）。解码始终在调用线程顺序进行。
    pub threads: usize,
    pub fusion: FusionParams,
    /// 可选的感知缓存与命名空间。命中则跳过检测。缓存键使用绝对采样序号，与扫描范围无关。
    pub cache: Option<(&'a PerceptualCache, &'a Namespace)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScanOutcome {
    Finished(Vec<FusedSegment>),
    /// 被取消。`scanned_until` 是最后处理过的时间，即续扫检查点。
    Cancelled { scanned_until: f64, partial: Vec<FusedSegment> },
}

/// 扫描起点对齐到采样网格后的 `(起点秒, 时长秒)`，用于打开 [`FfmpegFrameSource`]。
/// 范围为空时时长为 0。
pub fn sample_window(start: f64, end: f64, interval: f64) -> (f64, f64) {
    let (k0, k1) = sample_indices(start, end, interval);
    if k1 < k0 {
        return (k0 as f64 * interval, 0.0);
    }
    (k0 as f64 * interval, (k1 - k0 + 1) as f64 * interval)
}

/// 范围内的采样序号 `[k0, k1]`（两端都包含）。序号 k 对应时刻 `k * interval`。
fn sample_indices(start: f64, end: f64, interval: f64) -> (u64, u64) {
    let k0 = (start / interval - 1e-9).ceil().max(0.0) as u64;
    let k1 = (end / interval + 1e-9).floor().max(0.0) as u64;
    (k0, k1)
}

/// 一个采样帧的记录。`value` 为 `None` 表示结果尚未返回。
struct Slot<T> {
    t: f64,
    k: u64,
    value: Option<T>,
}

/// worker 返回的结果：任务序号与处理输出（或错误）。
type JobResult<T> = (usize, Result<T, String>);

/// 单帧处理器。每个检测线程通过工厂各自创建一个，因此不要求 `Send`。
pub trait FrameWorker<T> {
    fn process(&mut self, frame: &RgbFrame) -> Result<T, String>;
}

/// 逐帧结果的缓存读写。只在调用线程访问。
pub trait FrameStore<T> {
    fn get(&self, k: u64) -> Result<Option<T>, String>;
    fn put(&self, k: u64, value: &T) -> Result<(), String>;
}

/// 一次运行的输出。`frames` 包含所有已处理的采样帧（缓存命中与新计算的都有），按采样顺序排列。
pub struct FrameRun<T> {
    pub frames: Vec<(f64, T)>,
    pub cancelled: bool,
    /// 最后处理过的时刻；一帧都没有处理时为扫描起点。
    pub scanned_until: f64,
    /// 从缓存读取的帧数。
    pub reused: u64,
    /// 新计算的帧数。
    pub computed: u64,
}

/// 按采样点解码并逐帧处理的通用循环。
///
/// 解码在调用线程顺序进行，处理由 `cfg.threads` 个 worker 并行完成。结果按采样顺序汇总，
/// 因此与单线程结果一致。`store` 命中的帧直接复用，不送入 worker。`cfg.cache` 在这里不使用，
/// 缓存由调用方通过 `store` 指定。
///
/// 取消：标志被检查后不再读取新帧；已送入 worker 的帧会处理完并计入结果。
pub fn run_frames<T, S, F>(
    source: &mut S,
    make_worker: F,
    cfg: &ScanConfig<'_>,
    store: Option<&dyn FrameStore<T>>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f64),
) -> Result<FrameRun<T>, PipelineError>
where
    T: Send,
    S: FrameSource,
    F: Fn() -> Result<Box<dyn FrameWorker<T>>, String> + Sync,
{
    let threads = cfg.threads.max(1);
    let (k0, k1) = sample_indices(cfg.start, cfg.end, cfg.interval);
    // 有界任务队列：解码快于处理时阻塞解码端，限制内存占用。
    let (work_tx, work_rx) = mpsc::sync_channel::<(usize, RgbFrame)>(threads * 2);
    let work_rx = Mutex::new(work_rx);
    // 结果队列无界：worker 不会因为主线程忙而阻塞。
    let (res_tx, res_rx) = mpsc::channel::<JobResult<T>>();
    let make = &make_worker;
    let work_rx = &work_rx;

    thread::scope(|scope| -> Result<FrameRun<T>, PipelineError> {
        for _ in 0..threads {
            let res_tx = res_tx.clone();
            scope.spawn(move || {
                // 工作器在本线程内创建。创建失败时，每个任务都回报同一个错误。
                let mut worker = make();
                loop {
                    let job = match work_rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => break,
                    };
                    let Ok((seq, frame)) = job else { break };
                    let result = match worker.as_mut() {
                        Ok(w) => w.process(&frame),
                        Err(e) => Err(e.clone()),
                    };
                    if res_tx.send((seq, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(res_tx);

        let mut slots: Vec<Slot<T>> = Vec::new();
        let mut outstanding = 0usize;
        let mut reported = 0usize;
        let mut reused = 0u64;
        let mut computed = 0u64;
        let mut failure: Option<PipelineError> = None;
        let mut exhausted = false;
        let mut k = k0;

        loop {
            while let Ok((seq, result)) = res_rx.try_recv() {
                outstanding -= 1;
                store_result(&mut slots, seq, result, store, &mut failure);
            }
            report_progress(&slots, &mut reported, &mut on_progress);
            if failure.is_some() || cancel.load(Ordering::Relaxed) {
                break;
            }
            if k > k1 {
                exhausted = true;
                break;
            }

            // 干净提前结束（常见于 fps 滤镜取整）视为扫描完成，而不是错误。
            let frame = match source.next_frame() {
                Ok(Some(f)) => f,
                Ok(None) => {
                    exhausted = true;
                    break;
                }
                Err(e) => {
                    failure = Some(PipelineError::Source(e));
                    break;
                }
            };

            let seq = slots.len();
            let t = k as f64 * cfg.interval;
            let cached = match store {
                Some(s) => match s.get(k) {
                    Ok(v) => v,
                    Err(e) => {
                        failure = Some(PipelineError::Cache(e));
                        break;
                    }
                },
                None => None,
            };
            match cached {
                Some(value) => {
                    reused += 1;
                    slots.push(Slot { t, k, value: Some(value) });
                }
                None => {
                    computed += 1;
                    slots.push(Slot { t, k, value: None });
                    outstanding += 1;
                    if work_tx.send((seq, frame)).is_err() {
                        outstanding -= 1;
                        failure = Some(PipelineError::Detector("检测线程意外退出".into()));
                        break;
                    }
                }
            }
            k += 1;
        }

        // 关闭任务队列，等待已送出的帧全部返回，保证结果完整且线程能正常退出。
        drop(work_tx);
        while outstanding > 0 {
            let Ok((seq, result)) = res_rx.recv() else { break };
            outstanding -= 1;
            store_result(&mut slots, seq, result, store, &mut failure);
            report_progress(&slots, &mut reported, &mut on_progress);
        }
        report_progress(&slots, &mut reported, &mut on_progress);

        if let Some(e) = failure {
            return Err(e);
        }
        let cancelled = !exhausted && cancel.load(Ordering::Relaxed);
        let scanned_until = slots.last().map_or(cfg.start, |s| s.t);
        let frames = slots
            .into_iter()
            .filter_map(|s| {
                let t = s.t;
                s.value.map(|v| (t, v))
            })
            .collect();
        Ok(FrameRun { frames, cancelled, scanned_until, reused, computed })
    })
}

/// 把 worker 的结果写入对应槽位，并在结果出现时写回缓存。
fn store_result<T>(
    slots: &mut [Slot<T>],
    seq: usize,
    result: Result<T, String>,
    store: Option<&dyn FrameStore<T>>,
    failure: &mut Option<PipelineError>,
) {
    match result {
        Ok(value) => {
            if let Some(s) = store {
                if let Err(e) = s.put(slots[seq].k, &value) {
                    if failure.is_none() {
                        *failure = Some(PipelineError::Cache(e));
                    }
                }
            }
            slots[seq].value = Some(value);
        }
        Err(e) => {
            if failure.is_none() {
                *failure = Some(PipelineError::Detector(e));
            }
        }
    }
}

/// 按采样顺序回调进度：只有前面的帧都完成了才会报告后面的帧。
fn report_progress<T>(slots: &[Slot<T>], reported: &mut usize, on_progress: &mut impl FnMut(f64)) {
    while *reported < slots.len() && slots[*reported].value.is_some() {
        on_progress(slots[*reported].t);
        *reported += 1;
    }
}

/// 执行扫描（检测器输出）。
///
/// 解码在调用线程顺序进行，检测由 `cfg.threads` 个 worker 并行完成。每个 worker 通过
/// `make_detector` 在自己的线程里创建检测器，因为 ONNX 会话等对象不在线程间共享。
/// 结果按采样顺序汇总，因此与单线程的结果一致。`cfg.cache` 存放检测结果，命中则跳过检测。
///
/// `on_progress` 按采样顺序回调，参数为已完成的最新时刻。
///
/// 取消：标志被检查后不再读取新帧；已经送入 worker 的帧会处理完，并计入部分结果。
pub fn scan<S, F>(
    source: &mut S,
    make_detector: F,
    cfg: &ScanConfig<'_>,
    cancel: &AtomicBool,
    on_progress: impl FnMut(f64),
) -> Result<ScanOutcome, PipelineError>
where
    S: FrameSource,
    F: Fn() -> Result<Box<dyn Detector>, String> + Sync,
{
    let store = cfg.cache.map(|(cache, ns)| DetStore { cache, ns });
    let store_ref = store.as_ref().map(|s| s as &dyn FrameStore<Vec<Detection>>);
    let make = move || {
        make_detector().map(|d| Box::new(DetWorker(d)) as Box<dyn FrameWorker<Vec<Detection>>>)
    };
    let run = run_frames(source, make, cfg, store_ref, cancel, on_progress)?;
    let finished = !run.cancelled;
    let scanned_until = run.scanned_until;
    let segs = fuse_frames(run.frames, &cfg.fusion, cfg.interval, cfg.end, finished);
    Ok(if finished {
        ScanOutcome::Finished(segs)
    } else {
        ScanOutcome::Cancelled { scanned_until, partial: segs }
    })
}

/// 把逐帧结果融合为段。扫描完整时延伸段尾；取消时保持原样。
pub fn fuse_frames(
    frames: Vec<(f64, Vec<Detection>)>,
    fusion: &FusionParams,
    interval: f64,
    end: f64,
    finished: bool,
) -> Vec<FusedSegment> {
    let mut segs = fuse(frames, fusion);
    if finished {
        extend_segment_tails(&mut segs, interval, end);
    }
    segs
}

/// 采样只在离散时刻下结论：末次命中之后的一个采样间隔内仍按命中处理。
/// 隐私工具宁可多打一帧，也不能在段尾漏打；最后一段最多延伸到视频结尾。
fn extend_segment_tails(segs: &mut [FusedSegment], interval: f64, duration: f64) {
    for s in segs.iter_mut() {
        s.t1 = (s.t1 + interval).min(duration);
    }
}

/// 检测器适配为通用的帧处理器。
struct DetWorker(Box<dyn Detector>);

impl FrameWorker<Vec<Detection>> for DetWorker {
    fn process(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        self.0.detect(frame)
    }
}

/// 检测结果的感知缓存（按命名空间与采样序号）。
struct DetStore<'a> {
    cache: &'a PerceptualCache,
    ns: &'a Namespace,
}

impl FrameStore<Vec<Detection>> for DetStore<'_> {
    fn get(&self, k: u64) -> Result<Option<Vec<Detection>>, String> {
        self.cache.get_frame(self.ns, k).map_err(|e| e.to_string())
    }

    fn put(&self, k: u64, value: &Vec<Detection>) -> Result<(), String> {
        self.cache.put_frame(self.ns, k, value.as_slice()).map_err(|e| e.to_string())
    }
}

/// OCR 引擎的帧处理器：输出整帧的文字行，不做匹配。
pub struct OcrWorker<O: OcrEngine>(pub O);

impl<O: OcrEngine> FrameWorker<Vec<OcrLine>> for OcrWorker<O> {
    fn process(&mut self, frame: &RgbFrame) -> Result<Vec<OcrLine>, String> {
        self.0.recognize(frame.width, frame.height, &frame.data)
    }
}

/// OCR 文字行的感知缓存（按命名空间与采样序号）。
pub struct OcrStore<'a> {
    pub cache: &'a PerceptualCache,
    pub ns: &'a Namespace,
}

impl FrameStore<Vec<OcrLine>> for OcrStore<'_> {
    fn get(&self, k: u64) -> Result<Option<Vec<OcrLine>>, String> {
        self.cache.get_ocr_frame(self.ns, k).map_err(|e| e.to_string())
    }

    fn put(&self, k: u64, value: &Vec<OcrLine>) -> Result<(), String> {
        self.cache.put_ocr_frame(self.ns, k, value.as_slice()).map_err(|e| e.to_string())
    }
}

/// 在一帧的 OCR 文字行中查找匹配，给出框位与置信度。
///
/// `roi` 为 `(x, y, w, h)`，同为帧像素坐标，只保留中心点落在区域内的行；`None` 表示整帧。
/// 置信度低于 `min_conf` 的行被丢弃，与检测器阈值的语义一致。
pub fn match_lines(
    lines: &[OcrLine],
    query: &TextQuery,
    roi: Option<(f64, f64, f64, f64)>,
    min_conf: f64,
) -> Vec<Detection> {
    let mut out = Vec::new();
    for line in lines {
        if let Some((x, y, w, h)) = roi {
            let cx = line.quad.iter().map(|p| p[0]).sum::<f64>() / 4.0;
            let cy = line.quad.iter().map(|p| p[1]).sum::<f64>() / 4.0;
            if cx < x || cx >= x + w || cy < y || cy >= y + h {
                continue;
            }
        }
        if line.conf < min_conf {
            continue;
        }
        if let Some(hit) = query.find(&line.text) {
            let n = line.text.chars().count();
            out.push(Detection { quad: sub_quad(&line.quad, n, hit), conf: line.conf });
        }
    }
    out
}

/// 若扫描范围内每个采样帧都已有 OCR 缓存，按采样顺序返回 `(时刻, 文字行)`，此时不需要解码视频。
/// 只要有一帧缺失就返回 `None`。
pub fn cached_ocr_frames(
    cache: &PerceptualCache,
    ns: &Namespace,
    start: f64,
    end: f64,
    interval: f64,
) -> Result<Option<Vec<(f64, Vec<OcrLine>)>>, PipelineError> {
    let (k0, k1) = sample_indices(start, end, interval);
    if k1 < k0 {
        return Ok(Some(Vec::new()));
    }
    let expected = k1 - k0 + 1;
    let have = cache.ocr_frame_count_in(ns, k0, k1).map_err(|e| PipelineError::Cache(e.to_string()))?;
    if have < expected {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(expected as usize);
    for k in k0..=k1 {
        match cache.get_ocr_frame::<OcrLine>(ns, k).map_err(|e| PipelineError::Cache(e.to_string()))? {
            Some(lines) => out.push((k as f64 * interval, lines)),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// 把融合结果从代理分辨率映射回原视频分辨率（各向同性缩放时 `sx == sy`）。
pub fn scale_fused_segments(segs: &mut [FusedSegment], sx: f64, sy: f64) {
    for s in segs.iter_mut() {
        for k in s.keyframes.iter_mut() {
            for p in k.quad.iter_mut() {
                p[0] *= sx;
                p[1] *= sy;
            }
        }
    }
}

/// 把融合结果转换为项目里的检测段。新段都是待确认或低置信，只有用户能确认。
pub fn to_segments(target_id: &str, segs: &[FusedSegment], engine: Engine) -> Vec<Segment> {
    segs.iter()
        .enumerate()
        .map(|(i, s)| Segment {
            id: format!("{target_id}-seg{i}"),
            target: target_id.to_string(),
            t0: s.t0,
            t1: s.t1,
            status: if s.conf < LOW_CONFIDENCE { SegmentStatus::Low } else { SegmentStatus::Pending },
            source: Source::Auto,
            engine,
            conf: s.conf,
            keyframes: s
                .keyframes
                .iter()
                .map(|k| Keyframe { t: k.t, quad: k.quad, conf: k.conf })
                .collect(),
            negatives: Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_project::Quad;
    use vf_text::{MatchMode, OcrLine};

    /// 按固定顺序给出纯色帧。
    struct FakeSource {
        frames: usize,
        emitted: usize,
    }

    impl FrameSource for FakeSource {
        fn next_frame(&mut self) -> Result<Option<RgbFrame>, String> {
            if self.emitted >= self.frames {
                return Ok(None);
            }
            self.emitted += 1;
            Ok(Some(RgbFrame::new(8, 8, [0, 0, 0])))
        }
    }

    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    struct FakeOcr {
        calls: Arc<AtomicUsize>,
        lines: Vec<OcrLine>,
    }

    impl OcrEngine for FakeOcr {
        fn recognize(&mut self, _w: usize, _h: usize, _rgb: &[u8]) -> Result<Vec<OcrLine>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.lines.clone())
        }
    }

    fn line_quad() -> Quad {
        [[0.0, 0.0], [100.0, 0.0], [100.0, 10.0], [0.0, 10.0]]
    }

    /// 每个 worker 调用一次工厂，计数器在所有 worker 之间共享。
    fn text_detector(lines: Vec<OcrLine>, calls: Arc<AtomicUsize>) -> Result<Box<dyn Detector>, String> {
        Ok(Box::new(TextDetector {
            ocr: FakeOcr { calls, lines },
            query: TextQuery::new("机密", MatchMode::Contains, true).unwrap(),
        }))
    }

    fn cfg<'a>(
        start: f64,
        end: f64,
        threads: usize,
        cache: Option<(&'a PerceptualCache, &'a Namespace)>,
    ) -> ScanConfig<'a> {
        ScanConfig { start, end, interval: 0.5, threads, fusion: FusionParams::default(), cache }
    }

    fn match_line() -> Vec<OcrLine> {
        vec![OcrLine { quad: line_quad(), text: "机密".into(), conf: 0.9 }]
    }

    #[test]
    fn text_scan_yields_one_segment_with_sub_span_quad() {
        // "这里是机密文件" 共 7 个字符，"机密" 位于下标 3 与 4，即区间 [3, 5)
        let lines = vec![OcrLine { quad: line_quad(), text: "这里是机密文件".into(), conf: 0.9 }];
        let calls = Arc::new(AtomicUsize::new(0));
        let mut src = FakeSource { frames: 5, emitted: 0 };
        let cancel = AtomicBool::new(false);
        let factory = || text_detector(lines.clone(), calls.clone());
        let out = scan(&mut src, factory, &cfg(0.0, 2.0, 2, None), &cancel, |_| {}).unwrap();
        let ScanOutcome::Finished(segs) = out else { panic!("expected finished") };
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].t0, segs[0].t1), (0.0, 2.0));
        let x0 = segs[0].keyframes[0].quad[0][0];
        assert!((x0 - 100.0 * 3.0 / 7.0).abs() < 1e-9);
    }

    #[test]
    fn scanning_a_sub_range_only_samples_that_range() {
        // 范围 [1.0, 2.0]：采样点为 1.0、1.5、2.0，共 3 帧，检测段从 1.0 开始
        let calls = Arc::new(AtomicUsize::new(0));
        let mut src = FakeSource { frames: 3, emitted: 0 };
        let cancel = AtomicBool::new(false);
        let factory = || text_detector(match_line(), calls.clone());
        let out = scan(&mut src, factory, &cfg(1.0, 2.0, 1, None), &cancel, |_| {}).unwrap();
        let ScanOutcome::Finished(segs) = out else { panic!("expected finished") };
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].t0, 1.0);
        assert_eq!(segs[0].t1, 2.0, "尾部延长不能超过范围终点");
    }

    #[test]
    fn sample_window_aligns_start_to_grid() {
        assert_eq!(sample_window(1.2, 2.0, 0.5), (1.5, 1.0));
        assert_eq!(sample_window(0.0, 2.0, 0.5), (0.0, 2.5));
        assert_eq!(sample_window(3.0, 1.0, 0.5).1, 0.0);
    }

    #[test]
    fn thread_count_does_not_change_results() {
        let run = |threads: usize| {
            let calls = Arc::new(AtomicUsize::new(0));
            let mut src = FakeSource { frames: 12, emitted: 0 };
            let cancel = AtomicBool::new(false);
            let factory = || text_detector(match_line(), calls.clone());
            let out = scan(&mut src, factory, &cfg(0.0, 5.5, threads, None), &cancel, |_| {}).unwrap();
            (out, calls.load(Ordering::SeqCst))
        };
        let (single, _) = run(1);
        let (parallel, calls) = run(4);
        assert_eq!(single, parallel);
        assert_eq!(calls, 12);
    }

    #[test]
    fn cached_frames_skip_detection_on_second_scan() {
        let cache = PerceptualCache::open_in_memory().unwrap();
        let ns = Namespace::new("vid-hash", "ocr-v5", "{\"mode\":\"contains\"}");
        let calls = Arc::new(AtomicUsize::new(0));
        let cancel = AtomicBool::new(false);
        let factory = || text_detector(match_line(), calls.clone());

        let mut src = FakeSource { frames: 5, emitted: 0 };
        let first = scan(&mut src, factory, &cfg(0.0, 2.0, 2, Some((&cache, &ns))), &cancel, |_| {}).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 5);

        let mut src2 = FakeSource { frames: 5, emitted: 0 };
        let second = scan(&mut src2, factory, &cfg(0.0, 2.0, 2, Some((&cache, &ns))), &cancel, |_| {}).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 5, "第二次扫描应全部命中缓存");
        assert_eq!(first, second);
    }

    /// 源在输出第 3 帧时自行置位取消标志。取消在读取下一帧前生效，
    /// 因此前 3 帧一定完整处理，结果是确定的。
    struct CancelAfterSource<'c> {
        emitted: usize,
        cancel: &'c AtomicBool,
    }

    impl FrameSource for CancelAfterSource<'_> {
        fn next_frame(&mut self) -> Result<Option<RgbFrame>, String> {
            if self.emitted >= 9 {
                return Ok(None);
            }
            self.emitted += 1;
            if self.emitted == 3 {
                self.cancel.store(true, Ordering::Relaxed);
            }
            Ok(Some(RgbFrame::new(8, 8, [0, 0, 0])))
        }
    }

    #[test]
    fn cancel_keeps_partial_results_and_checkpoint() {
        let calls = Arc::new(AtomicUsize::new(0));
        let cancel = AtomicBool::new(false);
        let mut src = CancelAfterSource { emitted: 0, cancel: &cancel };
        let factory = || text_detector(match_line(), calls.clone());
        let out = scan(&mut src, factory, &cfg(0.0, 4.0, 3, None), &cancel, |_| {}).unwrap();
        match out {
            ScanOutcome::Cancelled { scanned_until, partial } => {
                assert_eq!(scanned_until, 1.0);
                assert_eq!(partial.len(), 1);
                assert_eq!(partial[0].t0, 0.0);
            }
            other => panic!("expected cancelled, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn clean_early_end_finishes_with_partial_results() {
        // ffmpeg 的 fps 滤镜在结尾可能少给一帧，这不应视为错误
        let calls = Arc::new(AtomicUsize::new(0));
        let mut src = FakeSource { frames: 2, emitted: 0 };
        let cancel = AtomicBool::new(false);
        let factory = || text_detector(match_line(), calls.clone());
        let out = scan(&mut src, factory, &cfg(0.0, 2.0, 2, None), &cancel, |_| {}).unwrap();
        let ScanOutcome::Finished(segs) = out else { panic!("expected finished") };
        assert_eq!(segs.len(), 1);
        assert!((segs[0].t1 - 1.0).abs() < 1e-9, "末次命中后应再覆盖一个采样间隔");
    }

    struct FixedDetector(Vec<Detection>);

    impl Detector for FixedDetector {
        fn detect(&mut self, _frame: &RgbFrame) -> Result<Vec<Detection>, String> {
            Ok(self.0.clone())
        }
    }

    fn det(x: f64, y: f64, conf: f64) -> Detection {
        Detection { quad: [[x, y], [x + 10.0, y], [x + 10.0, y + 10.0], [x, y + 10.0]], conf }
    }

    #[test]
    fn roi_detector_offsets_quads_and_crops_frame() {
        struct SizeDetector;
        impl Detector for SizeDetector {
            fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
                Ok(vec![Detection {
                    quad: [[0.0, 0.0], [frame.width as f64, 0.0], [frame.width as f64, frame.height as f64], [0.0, frame.height as f64]],
                    conf: 0.9,
                }])
            }
        }
        let mut roi = RoiDetector { inner: SizeDetector, roi: (5, 7, 10, 10) };
        let dets = roi.detect(&RgbFrame::new(40, 30, [0, 0, 0])).unwrap();
        assert_eq!(dets[0].quad[0], [5.0, 7.0]);
        assert_eq!(dets[0].quad[2], [15.0, 17.0]);
    }

    #[test]
    fn roi_detector_rejects_roi_outside_frame() {
        let mut roi = RoiDetector { inner: FixedDetector(vec![]), roi: (100, 0, 10, 10) };
        assert!(roi.detect(&RgbFrame::new(40, 30, [0, 0, 0])).is_err());
    }

    #[test]
    fn threshold_detector_filters_low_confidence() {
        let mut d = ThresholdDetector { inner: FixedDetector(vec![det(0.0, 0.0, 0.4), det(20.0, 0.0, 0.8)]), min_conf: 0.6 };
        let out = d.detect(&RgbFrame::new(40, 30, [0, 0, 0])).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].conf, 0.8);
    }

    #[test]
    fn multi_detector_concatenates_variants() {
        let mut d = MultiDetector {
            detectors: vec![
                Box::new(FixedDetector(vec![det(0.0, 0.0, 0.9)])),
                Box::new(FixedDetector(vec![det(50.0, 0.0, 0.8)])),
            ],
        };
        let out = d.detect(&RgbFrame::new(40, 30, [0, 0, 0])).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn scale_fused_segments_maps_proxy_to_source() {
        let mut segs = vec![FusedSegment {
            t0: 0.0,
            t1: 1.0,
            keyframes: vec![vf_fusion::FusedKeyframe { t: 0.0, quad: [[1.0, 2.0], [3.0, 4.0], [5.0, 6.0], [7.0, 8.0]], conf: 0.9 }],
            conf: 0.9,
        }];
        scale_fused_segments(&mut segs, 2.0, 0.5);
        assert_eq!(segs[0].keyframes[0].quad[1], [6.0, 2.0]);
    }

    #[test]
    fn low_confidence_segments_are_marked_low() {
        let fused = vec![
            FusedSegment { t0: 0.0, t1: 1.0, keyframes: vec![], conf: 0.4 },
            FusedSegment { t0: 2.0, t1: 3.0, keyframes: vec![], conf: 0.9 },
        ];
        let segs = to_segments("t1", &fused, Engine::Ocr);
        assert_eq!(segs[0].status, SegmentStatus::Low);
        assert_eq!(segs[1].status, SegmentStatus::Pending);
        assert_eq!(segs[0].source, Source::Auto);
        assert_eq!(segs[0].id, "t1-seg0");
    }
}

#[cfg(test)]
mod ocr_cache_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use vf_project::Quad;
    use vf_text::MatchMode;

    /// 按顺序产生 `n` 帧纯色图像。
    struct Frames {
        n: usize,
        emitted: usize,
    }

    impl FrameSource for Frames {
        fn next_frame(&mut self) -> Result<Option<RgbFrame>, String> {
            if self.emitted >= self.n {
                return Ok(None);
            }
            self.emitted += 1;
            Ok(Some(RgbFrame::new(8, 8, [0, 0, 0])))
        }
    }

    /// 计数的 OCR 引擎：每调用一次 `recognize` 计数加一。
    struct CountingOcr {
        calls: Arc<AtomicUsize>,
        lines: Vec<OcrLine>,
    }

    impl OcrEngine for CountingOcr {
        fn recognize(&mut self, _w: usize, _h: usize, _rgb: &[u8]) -> Result<Vec<OcrLine>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.lines.clone())
        }
    }

    fn counting_factory(
        calls: Arc<AtomicUsize>,
        lines: Vec<OcrLine>,
    ) -> impl Fn() -> Result<Box<dyn FrameWorker<Vec<OcrLine>>>, String> + Sync {
        move || {
            Ok(Box::new(OcrWorker(CountingOcr { calls: calls.clone(), lines: lines.clone() }))
                as Box<dyn FrameWorker<Vec<OcrLine>>>)
        }
    }

    /// 宽 50、高 10 的文字行，左上角 x 为 `x0`。
    fn line(text: &str, x0: f64, conf: f64) -> OcrLine {
        let quad: Quad = [[x0, 0.0], [x0 + 50.0, 0.0], [x0 + 50.0, 10.0], [x0, 10.0]];
        OcrLine { quad, text: text.into(), conf }
    }

    fn cfg(end: f64, interval: f64) -> ScanConfig<'static> {
        ScanConfig { start: 0.0, end, interval, threads: 2, fusion: FusionParams::default(), cache: None }
    }

    #[test]
    fn match_lines_applies_roi_and_threshold() {
        let lines = vec![line("机密A", 0.0, 0.9), line("机密B", 100.0, 0.4), line("机密C", 200.0, 0.8)];
        let q = TextQuery::new("机密", MatchMode::Contains, true).unwrap();
        // 整帧、阈值 0.5：B 的置信度低于阈值被丢弃
        assert_eq!(match_lines(&lines, &q, None, 0.5).len(), 2);
        // 区域只覆盖 x 在 [150, 300) 的行：只剩 C
        let roi = match_lines(&lines, &q, Some((150.0, 0.0, 150.0, 10.0)), 0.0);
        assert_eq!(roi.len(), 1);
        assert!((roi[0].conf - 0.8).abs() < 1e-12);
    }

    #[test]
    fn changing_pattern_reuses_cached_ocr_lines() {
        let cache = PerceptualCache::open_in_memory().unwrap();
        let ns = Namespace::new("vid", "ocr-ppv5-1", "{\"proxy\":[8,8]}");
        let calls = Arc::new(AtomicUsize::new(0));
        let lines = vec![line("机密文件", 0.0, 0.9), line("公开资料", 100.0, 0.9)];
        let store = OcrStore { cache: &cache, ns: &ns };
        let cancel = AtomicBool::new(false);

        let run = run_frames(
            &mut Frames { n: 3, emitted: 0 },
            counting_factory(calls.clone(), lines),
            &cfg(2.0, 1.0),
            Some(&store),
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(run.frames.len(), 3);
        assert_eq!((run.reused, run.computed), (0, 3));
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        // 缓存已覆盖整个范围：直接读取文字行，换两种模式串都不再调用 OCR
        let covered = cached_ocr_frames(&cache, &ns, 0.0, 2.0, 1.0).unwrap().expect("范围已覆盖");
        let q_old = TextQuery::new("机密", MatchMode::Contains, true).unwrap();
        let q_new = TextQuery::new("公开", MatchMode::Contains, true).unwrap();
        let hits_old: usize = covered.iter().map(|(_, l)| match_lines(l, &q_old, None, 0.0).len()).sum();
        let hits_new: usize = covered.iter().map(|(_, l)| match_lines(l, &q_new, None, 0.0).len()).sum();
        assert_eq!((hits_old, hits_new), (3, 3));
        assert_eq!(calls.load(Ordering::SeqCst), 3, "换模式串不应重新 OCR");
    }

    #[test]
    fn partial_coverage_is_not_treated_as_cached() {
        let cache = PerceptualCache::open_in_memory().unwrap();
        let ns = Namespace::new("vid", "ocr-ppv5-1", "p");
        let calls = Arc::new(AtomicUsize::new(0));
        let store = OcrStore { cache: &cache, ns: &ns };
        let cancel = AtomicBool::new(false);
        // 只扫描前两帧
        run_frames(
            &mut Frames { n: 2, emitted: 0 },
            counting_factory(calls.clone(), vec![line("机密", 0.0, 0.9)]),
            &cfg(1.0, 1.0),
            Some(&store),
            &cancel,
            |_| {},
        )
        .unwrap();
        assert!(cached_ocr_frames(&cache, &ns, 0.0, 2.0, 1.0).unwrap().is_none(), "第三帧缺失");
        assert!(cached_ocr_frames(&cache, &ns, 0.0, 1.0, 1.0).unwrap().is_some());
    }
}
