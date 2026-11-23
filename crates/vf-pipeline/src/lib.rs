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
use vf_text::{sub_quad, OcrEngine, TextQuery};
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

/// 一个采样帧的记录。`dets` 为 `None` 表示检测结果尚未返回。
struct Slot {
    t: f64,
    k: u64,
    dets: Option<Vec<Detection>>,
}

/// worker 返回的结果：任务序号与检测输出（或错误）。
type JobResult = (usize, Result<Vec<Detection>, String>);

/// 执行扫描。
///
/// 解码在调用线程顺序进行，检测由 `cfg.threads` 个 worker 并行完成。每个 worker 通过
/// `make_detector` 在自己的线程里创建检测器，因为 ONNX 会话等对象不在线程间共享。
/// 结果按采样顺序汇总，因此与单线程的结果一致。感知缓存只在调用线程读写。
///
/// `on_progress` 按采样顺序回调，参数为已完成的最新时刻。
///
/// 取消：标志被检查后不再读取新帧；已经送入 worker 的帧会处理完，并计入部分结果。
pub fn scan<S, F>(
    source: &mut S,
    make_detector: F,
    cfg: &ScanConfig<'_>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f64),
) -> Result<ScanOutcome, PipelineError>
where
    S: FrameSource,
    F: Fn() -> Result<Box<dyn Detector>, String> + Sync,
{
    let threads = cfg.threads.max(1);
    let (k0, k1) = sample_indices(cfg.start, cfg.end, cfg.interval);
    // 有界任务队列：解码快于检测时阻塞解码端，限制内存占用。
    let (work_tx, work_rx) = mpsc::sync_channel::<(usize, RgbFrame)>(threads * 2);
    let work_rx = Mutex::new(work_rx);
    // 结果队列无界：worker 不会因为主线程忙而阻塞。
    let (res_tx, res_rx) = mpsc::channel::<JobResult>();
    let make = &make_detector;
    let work_rx = &work_rx;

    thread::scope(|scope| -> Result<ScanOutcome, PipelineError> {
        for _ in 0..threads {
            let res_tx = res_tx.clone();
            scope.spawn(move || {
                // 检测器在本线程内创建。创建失败时，每个任务都回报同一个错误。
                let mut detector = make();
                loop {
                    let job = match work_rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => break,
                    };
                    let Ok((seq, frame)) = job else { break };
                    let result = match detector.as_mut() {
                        Ok(d) => d.detect(&frame),
                        Err(e) => Err(e.clone()),
                    };
                    if res_tx.send((seq, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(res_tx);

        let mut slots: Vec<Slot> = Vec::new();
        let mut outstanding = 0usize;
        let mut reported = 0usize;
        let mut failure: Option<PipelineError> = None;
        let mut exhausted = false;
        let mut k = k0;

        loop {
            while let Ok((seq, result)) = res_rx.try_recv() {
                outstanding -= 1;
                store_result(&mut slots, seq, result, cfg.cache, &mut failure);
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
            let cached = match cfg.cache {
                Some((cache, ns)) => match cache.get_frame(ns, k) {
                    Ok(v) => v,
                    Err(e) => {
                        failure = Some(PipelineError::Cache(e.to_string()));
                        break;
                    }
                },
                None => None,
            };
            match cached {
                Some(dets) => slots.push(Slot { t, k, dets: Some(dets) }),
                None => {
                    slots.push(Slot { t, k, dets: None });
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
            store_result(&mut slots, seq, result, cfg.cache, &mut failure);
            report_progress(&slots, &mut reported, &mut on_progress);
        }
        report_progress(&slots, &mut reported, &mut on_progress);

        if let Some(e) = failure {
            return Err(e);
        }
        if !exhausted && cancel.load(Ordering::Relaxed) {
            let scanned_until = slots.last().map_or(cfg.start, |s| s.t);
            let partial = fuse(into_results(slots), &cfg.fusion);
            return Ok(ScanOutcome::Cancelled { scanned_until, partial });
        }

        let mut segs = fuse(into_results(slots), &cfg.fusion);
        extend_segment_tails(&mut segs, cfg.interval, cfg.end);
        Ok(ScanOutcome::Finished(segs))
    })
}

/// 把 worker 的结果写入对应槽位，并在检测结果出现时写回感知缓存。
fn store_result(
    slots: &mut [Slot],
    seq: usize,
    result: Result<Vec<Detection>, String>,
    cache: Option<(&PerceptualCache, &Namespace)>,
    failure: &mut Option<PipelineError>,
) {
    match result {
        Ok(dets) => {
            if let Some((c, ns)) = cache {
                if let Err(e) = c.put_frame(ns, slots[seq].k, &dets) {
                    if failure.is_none() {
                        *failure = Some(PipelineError::Cache(e.to_string()));
                    }
                }
            }
            slots[seq].dets = Some(dets);
        }
        Err(e) => {
            if failure.is_none() {
                *failure = Some(PipelineError::Detector(e));
            }
        }
    }
}

/// 按采样顺序回调进度：只有前面的帧都完成了才会报告后面的帧。
fn report_progress(slots: &[Slot], reported: &mut usize, on_progress: &mut impl FnMut(f64)) {
    while *reported < slots.len() && slots[*reported].dets.is_some() {
        on_progress(slots[*reported].t);
        *reported += 1;
    }
}

/// 取出已完成的 `(时刻, 检测结果)`，顺序与采样顺序一致。
fn into_results(slots: Vec<Slot>) -> Vec<(f64, Vec<Detection>)> {
    slots
        .into_iter()
        .filter_map(|s| {
            let t = s.t;
            s.dets.map(|d| (t, d))
        })
        .collect()
}

/// 采样只在离散时刻下结论：末次命中之后的一个采样间隔内仍按命中处理。
/// 隐私工具宁可多打一帧，也不能在段尾漏打；最后一段最多延伸到视频结尾。
fn extend_segment_tails(segs: &mut [FusedSegment], interval: f64, duration: f64) {
    for s in segs.iter_mut() {
        s.t1 = (s.t1 + interval).min(duration);
    }
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
