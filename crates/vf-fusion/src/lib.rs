//! 检测–跟踪融合（对应 `first_design.md` 2.5）。
//!
//! 输入为按时间排列的逐帧检测结果，输出为检测段。规则：
//! - 同一目标在相邻检测帧之间用 IoU 关联；
//! - 连续命中 `open_hits` 次后开段；
//! - 连续丢失 `close_misses` 次后关段，中间的短暂丢失视为补洞；
//! - 四角坐标用指数滑动平均（EMA）平滑。
//!
//! 这里的 IoU 基于四边形的外接矩形，是对真实多边形 IoU 的近似（见注释）。

use serde::{Deserialize, Serialize};
use vf_project::Quad;

/// 单帧中的一次检测。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    pub quad: Quad,
    pub conf: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionParams {
    /// 连续命中多少次后开段。
    pub open_hits: usize,
    /// 连续丢失多少次后关段。
    pub close_misses: usize,
    /// IoU 关联阈值。
    pub iou_threshold: f64,
    /// 帧内 NMS 阈值。
    pub nms_threshold: f64,
    /// EMA 平滑系数，越大越跟随新观测。
    pub smooth_alpha: f64,
}

impl Default for FusionParams {
    fn default() -> Self {
        Self { open_hits: 2, close_misses: 3, iou_threshold: 0.3, nms_threshold: 0.5, smooth_alpha: 0.6 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FusedKeyframe {
    pub t: f64,
    pub quad: Quad,
    pub conf: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FusedSegment {
    pub t0: f64,
    pub t1: f64,
    pub keyframes: Vec<FusedKeyframe>,
    /// 段级置信度：关键帧置信度的均值。
    pub conf: f64,
}

/// 四边形的外接矩形 `[x0, y0, x1, y1]`。
pub fn bounding_box(q: &Quad) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in q {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

/// 外接矩形的 IoU。
pub fn iou(a: &Quad, b: &Quad) -> f64 {
    let (a, b) = (bounding_box(a), bounding_box(b));
    let iw = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let ih = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let area = |r: [f64; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// 按置信度从高到低做非极大值抑制。
pub fn nms(mut dets: Vec<Detection>, threshold: f64) -> Vec<Detection> {
    dets.sort_by(|a, b| b.conf.partial_cmp(&a.conf).unwrap_or(std::cmp::Ordering::Equal));
    let mut kept: Vec<Detection> = Vec::new();
    for d in dets {
        if kept.iter().all(|k| iou(&k.quad, &d.quad) < threshold) {
            kept.push(d);
        }
    }
    kept
}

fn smooth(old: &Quad, new: &Quad, alpha: f64) -> Quad {
    let mut out = *old;
    for i in 0..4 {
        for c in 0..2 {
            out[i][c] = alpha * new[i][c] + (1.0 - alpha) * old[i][c];
        }
    }
    out
}

struct Track {
    quad: Quad,
    misses: usize,
    keys: Vec<FusedKeyframe>,
    open: bool,
}

fn finish(track: Track) -> FusedSegment {
    let t0 = track.keys.first().map(|k| k.t).unwrap_or(0.0);
    let t1 = track.keys.last().map(|k| k.t).unwrap_or(0.0);
    let conf = track.keys.iter().map(|k| k.conf).sum::<f64>() / track.keys.len().max(1) as f64;
    FusedSegment { t0, t1, keyframes: track.keys, conf }
}

/// 融合逐帧检测。`frames` 必须按时间递增排列。
pub fn fuse<I>(frames: I, params: &FusionParams) -> Vec<FusedSegment>
where
    I: IntoIterator<Item = (f64, Vec<Detection>)>,
{
    let mut tracks: Vec<Track> = Vec::new();
    let mut out: Vec<FusedSegment> = Vec::new();

    for (t, dets) in frames {
        let dets = nms(dets, params.nms_threshold);
        let mut used = vec![false; dets.len()];

        for tr in tracks.iter_mut() {
            let mut best: Option<(usize, f64)> = None;
            for (i, d) in dets.iter().enumerate() {
                if used[i] {
                    continue;
                }
                let v = iou(&tr.quad, &d.quad);
                if v >= params.iou_threshold && best.map_or(true, |(_, b)| v > b) {
                    best = Some((i, v));
                }
            }
            match best {
                Some((i, _)) => {
                    used[i] = true;
                    tr.quad = smooth(&tr.quad, &dets[i].quad, params.smooth_alpha);
                    tr.misses = 0;
                    tr.keys.push(FusedKeyframe { t, quad: tr.quad, conf: dets[i].conf });
                    if !tr.open && tr.keys.len() >= params.open_hits {
                        tr.open = true;
                    }
                }
                None => tr.misses += 1,
            }
        }

        // 关闭丢失过久的轨迹；未开段的候选直接丢弃。
        let mut still = Vec::with_capacity(tracks.len());
        for tr in tracks.drain(..) {
            if tr.misses >= params.close_misses {
                if tr.open {
                    out.push(finish(tr));
                }
            } else {
                still.push(tr);
            }
        }
        tracks = still;

        // 未被匹配的检测开始新的候选轨迹。
        for (i, d) in dets.into_iter().enumerate() {
            if used[i] {
                continue;
            }
            let keys = vec![FusedKeyframe { t, quad: d.quad, conf: d.conf }];
            let open = params.open_hits <= 1;
            tracks.push(Track { quad: d.quad, misses: 0, keys, open });
        }
    }

    for tr in tracks {
        if tr.open {
            out.push(finish(tr));
        }
    }
    out.sort_by(|a, b| a.t0.partial_cmp(&b.t0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(x: f64, y: f64) -> Quad {
        [[x, y], [x + 10.0, y], [x + 10.0, y + 10.0], [x, y + 10.0]]
    }

    fn det(x: f64, y: f64) -> Detection {
        Detection { quad: sq(x, y), conf: 0.9 }
    }

    fn run(frames: Vec<(f64, Vec<Detection>)>) -> Vec<FusedSegment> {
        fuse(frames, &FusionParams::default())
    }

    #[test]
    fn continuous_detections_form_one_segment() {
        let frames = (0..10).map(|i| (i as f64, vec![det(0.0, 0.0)])).collect();
        let segs = run(frames);
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].t0, segs[0].t1), (0.0, 9.0));
        assert_eq!(segs[0].keyframes.len(), 10);
    }

    #[test]
    fn single_hit_never_opens_a_segment() {
        let segs = run(vec![(0.0, vec![det(0.0, 0.0)]), (1.0, vec![]), (2.0, vec![]), (3.0, vec![])]);
        assert!(segs.is_empty());
    }

    #[test]
    fn short_gap_is_bridged() {
        // 丢失 2 帧，小于 close_misses=3，应合并为一段
        let mut frames: Vec<(f64, Vec<Detection>)> = Vec::new();
        for i in 0..3 {
            frames.push((i as f64, vec![det(0.0, 0.0)]));
        }
        frames.push((3.0, vec![]));
        frames.push((4.0, vec![]));
        for i in 5..8 {
            frames.push((i as f64, vec![det(0.0, 0.0)]));
        }
        let segs = run(frames);
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].t0, segs[0].t1), (0.0, 7.0));
    }

    #[test]
    fn long_gap_splits_segments() {
        // 连续丢失 3 帧，达到 close_misses，应分成两段
        let mut frames: Vec<(f64, Vec<Detection>)> = Vec::new();
        for i in 0..3 {
            frames.push((i as f64, vec![det(0.0, 0.0)]));
        }
        for i in 3..6 {
            frames.push((i as f64, vec![]));
        }
        for i in 6..9 {
            frames.push((i as f64, vec![det(0.0, 0.0)]));
        }
        let segs = run(frames);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].t1, 2.0);
        assert_eq!(segs[1].t0, 6.0);
    }

    #[test]
    fn nms_removes_duplicates_keeping_highest_confidence() {
        let dets = vec![
            Detection { quad: sq(0.0, 0.0), conf: 0.6 },
            Detection { quad: sq(0.5, 0.0), conf: 0.9 },
        ];
        let kept = nms(dets, 0.5);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].conf, 0.9);
    }

    #[test]
    fn iou_of_identical_and_disjoint_quads() {
        assert!((iou(&sq(0.0, 0.0), &sq(0.0, 0.0)) - 1.0).abs() < 1e-9);
        assert_eq!(iou(&sq(0.0, 0.0), &sq(100.0, 100.0)), 0.0);
    }

    #[test]
    fn ema_smooths_a_jump() {
        let frames = vec![
            (0.0, vec![det(0.0, 0.0)]),
            (1.0, vec![det(0.0, 0.0)]),
            (2.0, vec![det(2.0, 0.0)]),
        ];
        let segs = run(frames);
        let last = segs[0].keyframes.last().unwrap().quad[0][0];
        // 旧值 0，新观测 2，alpha=0.6 → 1.2（未直接跳到 2）
        assert!((last - 1.2).abs() < 1e-9);
    }
}
