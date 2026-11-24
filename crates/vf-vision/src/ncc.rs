//! 极速档：OpenCV `matchTemplate`（TM_CCOEFF_NORMED，即归一化互相关 NCC）。
//!
//! 对应 `first_design.md` 2.2 的"多尺度 NCC 模板匹配"。NCC 单独使用时容易误报：
//! 低方差的模板或窗口上 NCC 不稳定，相似的文字块也能拿到较高分。因此本模块提供：
//! - 模板方差检查：平坦模板直接拒绝；
//! - 候选窗口方差检查：窗口基本是平的也拒绝；
//! - 跨尺度的全局非极大值抑制，以及全局候选数上限。
//! 误报的最终压制由 `vf-pipeline` 的级联检测器完成：候选还要通过局部特征匹配确认。

use std::cmp::Ordering;

use opencv::{core, imgproc, prelude::*, Result};
use vf_project::Quad;

/// 模板标准差低于此值（灰度 0–255）视为平坦，无法可靠定位。
pub const MIN_TEMPLATE_STD: f64 = 10.0;
/// 候选窗口标准差低于模板标准差的这一比例时拒绝（窗口基本是平的）。
pub const MIN_WINDOW_STD_RATIO: f64 = 0.5;
/// 两个候选框的 IoU 超过此值，视为同一目标。
const NMS_IOU: f64 = 0.3;

/// 一次命中。`x, y, w, h` 为帧内矩形（帧像素坐标）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NccMatch {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub score: f64,
    pub scale: f64,
}

impl NccMatch {
    pub fn quad(&self) -> Quad {
        let (x, y) = (self.x as f64, self.y as f64);
        let (w, h) = (self.w as f64, self.h as f64);
        [[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
    }

    /// 两个矩形的交并比。
    pub fn iou(&self, other: &NccMatch) -> f64 {
        let (ax1, ay1) = (self.x as f64, self.y as f64);
        let (ax2, ay2) = (ax1 + self.w as f64, ay1 + self.h as f64);
        let (bx1, by1) = (other.x as f64, other.y as f64);
        let (bx2, by2) = (bx1 + other.w as f64, by1 + other.h as f64);
        let iw = (ax2.min(bx2) - ax1.max(bx1)).max(0.0);
        let ih = (ay2.min(by2) - ay1.max(by1)).max(0.0);
        let inter = iw * ih;
        let union = (ax2 - ax1) * (ay2 - ay1) + (bx2 - bx1) * (by2 - by1) - inter;
        if union <= 0.0 {
            0.0
        } else {
            inter / union
        }
    }
}

/// 单通道图像的标准差。
pub fn template_std(gray: &core::Mat) -> Result<f64> {
    let mut mean = core::Mat::default();
    let mut sd = core::Mat::default();
    core::mean_std_dev(gray, &mut mean, &mut sd, &core::no_array())?;
    Ok(*sd.at::<f64>(0)?)
}

pub struct NccTemplate {
    gray: core::Mat,
    scales: Vec<f64>,
    threshold: f64,
    max_matches: usize,
}

impl NccTemplate {
    /// `template_gray` 为单通道模板，已经是帧所在分辨率。`scales` 为多尺度比例（如 `[0.9, 1.0, 1.1]`）。
    /// `threshold` 为 NCC 分数下限；`max_matches` 为每次调用返回的候选数上限（跨尺度合计）。
    pub fn new(template_gray: core::Mat, scales: Vec<f64>, threshold: f64, max_matches: usize) -> Self {
        Self { gray: template_gray, scales, threshold, max_matches }
    }

    /// 模板的标准差，用于判断是否平坦。
    pub fn template_std(&self) -> Result<f64> {
        template_std(&self.gray)
    }

    /// 在 `frame_gray` 的 `roi` 内搜索，返回按分数降序、经非极大值抑制的候选。
    pub fn find(&self, frame_gray: &core::Mat, roi: core::Rect) -> Result<Vec<NccMatch>> {
        let tpl_std = template_std(&self.gray)?;
        if tpl_std < MIN_TEMPLATE_STD {
            return Ok(Vec::new());
        }
        let search = frame_gray.roi(roi)?.try_clone()?;
        let mut cands: Vec<NccMatch> = Vec::new();
        for &s in &self.scales {
            let mut tpl = core::Mat::default();
            if (s - 1.0).abs() < 1e-9 {
                tpl = self.gray.try_clone()?;
            } else {
                imgproc::resize(&self.gray, &mut tpl, core::Size::new(0, 0), s, s, imgproc::INTER_LINEAR)?;
            }
            if tpl.rows() < 2 || tpl.cols() < 2 || tpl.rows() > search.rows() || tpl.cols() > search.cols() {
                continue;
            }
            let (tw, th) = (tpl.cols(), tpl.rows());
            let mut result = core::Mat::default();
            imgproc::match_template(&search, &tpl, &mut result, imgproc::TM_CCOEFF_NORMED, &core::no_array())?;
            let (rw, rh) = (result.cols(), result.rows());
            for _ in 0..self.max_matches {
                let mut max_val = 0f64;
                let mut max_loc = core::Point::default();
                core::min_max_loc(&result, None, Some(&mut max_val), None, Some(&mut max_loc), &core::no_array())?;
                if max_val < self.threshold {
                    break;
                }
                // 候选窗口的方差检查：窗口几乎是平的，NCC 分数没有意义
                let win = frame_gray.roi(core::Rect::new(roi.x + max_loc.x, roi.y + max_loc.y, tw, th))?.try_clone()?;
                let win_std = template_std(&win)?;
                if win_std >= MIN_WINDOW_STD_RATIO * tpl_std {
                    cands.push(NccMatch {
                        x: roi.x + max_loc.x,
                        y: roi.y + max_loc.y,
                        w: tw,
                        h: th,
                        score: max_val,
                        scale: s,
                    });
                }
                // 无论是否通过方差检查，都抑制该峰值附近，继续寻找下一个峰值
                let x0 = (max_loc.x - tw / 2).max(0);
                let y0 = (max_loc.y - th / 2).max(0);
                let x1 = (max_loc.x + tw / 2 + 1).min(rw);
                let y1 = (max_loc.y + th / 2 + 1).min(rh);
                let mut sub = result.roi_mut(core::Rect::new(x0, y0, x1 - x0, y1 - y0))?;
                sub.set_to(&core::Scalar::all(-1.0), &core::no_array())?;
            }
        }

        // 全局非极大值抑制：跨尺度的重复框只保留分数最高的一个
        cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        let mut kept: Vec<NccMatch> = Vec::new();
        for c in cands {
            if kept.iter().all(|k| k.iou(&c) < NMS_IOU) {
                kept.push(c);
                if kept.len() >= self.max_matches {
                    break;
                }
            }
        }
        Ok(kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{proxy_frame, proxy_obj, proxy_template, source_scene};

    fn full(frame: &core::Mat) -> core::Rect {
        core::Rect::new(0, 0, frame.cols(), frame.rows())
    }

    #[test]
    fn flat_template_is_rejected() -> Result<()> {
        let frame = proxy_frame(&source_scene(false)?)?;
        let flat = core::Mat::new_rows_cols_with_default(40, 60, core::CV_8UC1, core::Scalar::all(240.0))?;
        let ncc = NccTemplate::new(flat, vec![1.0], 0.5, 5);
        assert!(ncc.find(&frame, full(&frame))?.is_empty(), "平坦模板不应产生任何命中");
        Ok(())
    }

    #[test]
    fn spurious_flat_peaks_are_removed_by_threshold() -> Result<()> {
        // 旧实现在阈值 0 时会把 0.62 左右的平坦区域峰值全部保留；现在阈值 0.8 下只剩真实目标
        let src = source_scene(false)?;
        let frame = proxy_frame(&src)?;
        let tpl = proxy_template(&src)?;
        let ncc = NccTemplate::new(tpl, vec![0.9, 1.0, 1.1], 0.8, 5);
        let hits = ncc.find(&frame, full(&frame))?;
        assert_eq!(hits.len(), 1, "应只剩真实目标，实际 {hits:?}");
        let (ox, oy, _, _) = proxy_obj();
        assert!((hits[0].x as f64 - ox).abs() <= 3.0 && (hits[0].y as f64 - oy).abs() <= 3.0, "命中位置 {hits:?}");
        assert!(hits[0].score > 0.9);
        Ok(())
    }

    #[test]
    fn cross_scale_duplicates_collapse_to_one() -> Result<()> {
        let src = source_scene(false)?;
        let frame = proxy_frame(&src)?;
        let tpl = proxy_template(&src)?;
        // 阈值放低，三个尺度都会在目标上给出峰值；非极大值抑制后应合并为一个
        let ncc = NccTemplate::new(tpl, vec![0.9, 1.0, 1.1], 0.3, 20);
        let hits = ncc.find(&frame, full(&frame))?;
        let near: Vec<_> = hits.iter().filter(|h| (h.x as f64 - 400.0).abs() < 10.0 && (h.y as f64 - 200.0).abs() < 10.0).collect();
        assert_eq!(near.len(), 1, "同一目标的跨尺度重复框应被合并，实际 {near:?}");
        Ok(())
    }

    #[test]
    fn match_limit_caps_candidates() -> Result<()> {
        let src = source_scene(false)?;
        let frame = proxy_frame(&src)?;
        let tpl = proxy_template(&src)?;
        let ncc = NccTemplate::new(tpl, vec![1.0], 0.0, 2);
        assert!(ncc.find(&frame, full(&frame))?.len() <= 2);
        Ok(())
    }
}