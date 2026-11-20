//! 扫描流水线（对应 `first_design.md` 2.2、2.5、2.8）。
//!
//! 流程：按采样间隔顺序抽帧 → 检测（先查感知缓存）→ 时序融合为检测段。
//! 取消通过原子标志实现，取消后返回已扫描部分的结果与检查点（续扫用）。
//!
//! 检测器：
//! - `TextDetector`：OCR（PP-OCRv5）+ 文字匹配（`vf-text`）。
//! - `SiftDetector`：SIFT + RANSAC 单应（标准档）。
//! - `NccDetector`：`matchTemplate` 多尺度 NCC（极速档）。

use std::sync::atomic::{AtomicBool, Ordering};

use opencv::core;
use vf_cache::{Namespace, PerceptualCache};
use vf_fusion::{fuse, Detection, FusedSegment, FusionParams};
use vf_image::{gray_from_rgb, RgbFrame};
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
    /// 搜索区域 `(x, y, w, h)`。
    pub roi: (i32, i32, i32, i32),
}

impl Detector for NccDetector {
    fn detect(&mut self, frame: &RgbFrame) -> Result<Vec<Detection>, String> {
        let gray = gray_from_rgb(frame.width, frame.height, &frame.data).map_err(|e| e.to_string())?;
        let (x, y, w, h) = self.roi;
        let roi = core::Rect::new(x, y, w, h);
        let matches = self.template.find(&gray, roi).map_err(|e| e.to_string())?;
        Ok(matches.iter().map(|m| Detection { quad: m.quad(), conf: m.score }).collect())
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
        let frame = source
            .next_frame()
            .map_err(PipelineError::Source)?
            .ok_or_else(|| PipelineError::Source(format!("视频在第 {i} 帧提前结束")))?;

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

    Ok(ScanOutcome::Finished(fuse(results, &cfg.fusion)))
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
    fn short_source_is_a_source_error() {
        let mut det = text_detector(vec![]);
        let mut src = FakeSource { frames: 2, emitted: 0 };
        let cancel = AtomicBool::new(false);
        let err = scan(&mut src, &mut det, &cfg(2.0, None), &cancel, |_| {}).unwrap_err();
        assert!(matches!(err, PipelineError::Source(_)));
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
