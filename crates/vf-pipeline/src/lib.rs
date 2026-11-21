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
    pub fn open(ffmpeg: &Path, input: &Path, fps: f64, size: Option<(usize, usize)>) -> Result<Self, String> {
        let stream = FrameStream::open(ffmpeg, input, Some(fps), size).map_err(|e| e.to_string())?;
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

/// 扫描参数。
pub struct ScanConfig<'a> {
    pub duration: f64,
    /// 采样间隔（秒）。`FrameSource` 必须按同样的间隔输出帧。
    pub interval: f64,
    pub fusion: FusionParams,
    /// 可选的感知缓存与命名空间。命中则跳过检测。
    pub cache: Option<(&'a PerceptualCache, &'a Namespace)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScanOutcome {
    Finished(Vec<FusedSegment>),
    /// 被取消。`scanned_until` 是最后处理过的时间，即续扫检查点。
    Cancelled { scanned_until: f64, partial: Vec<FusedSegment> },
}

/// 执行扫描。`on_progress` 在每帧处理后调用，参数为当前帧的时间。
pub fn scan<S, D>(
    source: &mut S,
    detector: &mut D,
    cfg: &ScanConfig<'_>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f64),
) -> Result<ScanOutcome, PipelineError>
where
    S: FrameSource,
    D: Detector,
{
    let count = (cfg.duration / cfg.interval).floor() as u64 + 1;
    let mut results: Vec<(f64, Vec<Detection>)> = Vec::new();

    for i in 0..count {
        let t = i as f64 * cfg.interval;
        if t > cfg.duration + 1e-9 {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            let scanned_until = results.last().map_or(0.0, |(t, _)| *t);
            return Ok(ScanOutcome::Cancelled { scanned_until, partial: fuse(results, &cfg.fusion) });
        }

        // 顺序解码必须始终推进，即使命中缓存也要读取，保证帧与时间对齐。
        // 干净提前结束（常见于 fps 滤镜取整）视为扫描完成，而不是错误。
        let Some(frame) = source.next_frame().map_err(PipelineError::Source)? else { break };

        let cached = match cfg.cache {
            Some((cache, ns)) => cache.get_frame(ns, i).map_err(|e| PipelineError::Cache(e.to_string()))?,
            None => None,
        };
        let dets = match cached {
            Some(d) => d,
            None => {
                let d = detector.detect(&frame).map_err(PipelineError::Detector)?;
                if let Some((cache, ns)) = cfg.cache {
                    cache.put_frame(ns, i, &d).map_err(|e| PipelineError::Cache(e.to_string()))?;
                }
                d
            }
        };
        results.push((t, dets));
        on_progress(t);
    }

    let mut segs = fuse(results, &cfg.fusion);
    extend_segment_tails(&mut segs, cfg.interval, cfg.duration);
    Ok(ScanOutcome::Finished(segs))
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

    struct FakeOcr {
        calls: usize,
        lines: Vec<OcrLine>,
    }

    impl OcrEngine for FakeOcr {
        fn recognize(&mut self, _w: usize, _h: usize, _rgb: &[u8]) -> Result<Vec<OcrLine>, String> {
            self.calls += 1;
            Ok(self.lines.clone())
        }
    }

    fn line_quad() -> Quad {
        [[0.0, 0.0], [100.0, 0.0], [100.0, 10.0], [0.0, 10.0]]
    }

    fn text_detector(lines: Vec<OcrLine>) -> TextDetector<FakeOcr> {
        TextDetector {
            ocr: FakeOcr { calls: 0, lines },
            query: TextQuery::new("机密", MatchMode::Contains, true).unwrap(),
        }
    }

    fn cfg<'a>(duration: f64, cache: Option<(&'a PerceptualCache, &'a Namespace)>) -> ScanConfig<'a> {
        ScanConfig { duration, interval: 0.5, fusion: FusionParams::default(), cache }
    }

    #[test]
    fn text_scan_yields_one_segment_with_sub_span_quad() {
        // "这里是机密文件" 共 7 个字符，"机密" 位于下标 3 与 4，即区间 [3, 5)
        let mut det = text_detector(vec![OcrLine { quad: line_quad(), text: "这里是机密文件".into(), conf: 0.9 }]);
        let mut src = FakeSource { frames: 5, emitted: 0 };
        let cancel = AtomicBool::new(false);
        let out = scan(&mut src, &mut det, &cfg(2.0, None), &cancel, |_| {}).unwrap();
        let ScanOutcome::Finished(segs) = out else { panic!("expected finished") };
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].t0, segs[0].t1), (0.0, 2.0));
        let x0 = segs[0].keyframes[0].quad[0][0];
        assert!((x0 - 100.0 * 3.0 / 7.0).abs() < 1e-9);
    }

    #[test]
    fn cached_frames_skip_detection_on_second_scan() {
        let cache = PerceptualCache::open_in_memory().unwrap();
        let ns = Namespace::new("vid-hash", "ocr-v5", "{\"mode\":\"contains\"}");
        let mut det = text_detector(vec![OcrLine { quad: line_quad(), text: "机密".into(), conf: 0.9 }]);
        let cancel = AtomicBool::new(false);

        let mut src = FakeSource { frames: 5, emitted: 0 };
        let first = scan(&mut src, &mut det, &cfg(2.0, Some((&cache, &ns))), &cancel, |_| {}).unwrap();
        assert_eq!(det.ocr.calls, 5);

        let mut src2 = FakeSource { frames: 5, emitted: 0 };
        let second = scan(&mut src2, &mut det, &cfg(2.0, Some((&cache, &ns))), &cancel, |_| {}).unwrap();
        assert_eq!(det.ocr.calls, 5, "第二次扫描应全部命中缓存");
        assert_eq!(first, second);
    }

    #[test]
    fn cancel_keeps_partial_results_and_checkpoint() {
        let mut det = text_detector(vec![OcrLine { quad: line_quad(), text: "机密".into(), conf: 0.9 }]);
        let mut src = FakeSource { frames: 9, emitted: 0 };
        let cancel = AtomicBool::new(false);
        let out = scan(&mut src, &mut det, &cfg(4.0, None), &cancel, |t| {
            if t >= 1.0 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
        match out {
            ScanOutcome::Cancelled { scanned_until, partial } => {
                assert_eq!(scanned_until, 1.0);
                assert_eq!(partial.len(), 1);
                assert_eq!(partial[0].t0, 0.0);
            }
            other => panic!("expected cancelled, got {other:?}"),
        }
    }

    #[test]
    fn clean_early_end_finishes_with_partial_results() {
        // ffmpeg 的 fps 滤镜在结尾可能少给一帧，这不应视为错误
        let mut det = text_detector(vec![OcrLine { quad: line_quad(), text: "机密".into(), conf: 0.9 }]);
        let mut src = FakeSource { frames: 2, emitted: 0 };
        let cancel = AtomicBool::new(false);
        let out = scan(&mut src, &mut det, &cfg(2.0, None), &cancel, |_| {}).unwrap();
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
