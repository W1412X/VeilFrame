//! 极速档：OpenCV `matchTemplate`（TM_CCOEFF_NORMED，即归一化互相关 NCC）。
//! 对应 `first_design.md` 2.2 的"多尺度 NCC 模板匹配"，适合近静态、不透明的台标与水印。

use opencv::{core, imgproc, prelude::*, Result};
use vf_project::Quad;

/// 一次命中。`x, y, w, h` 为原帧坐标下的矩形。
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
}

pub struct NccTemplate {
    gray: core::Mat,
    scales: Vec<f64>,
    threshold: f64,
    max_matches: usize,
}

impl NccTemplate {
    /// `template_gray` 为单通道模板。`scales` 为多尺度比例（如 `[0.8, 1.0, 1.25]`）。
    pub fn new(template_gray: core::Mat, scales: Vec<f64>, threshold: f64, max_matches: usize) -> Self {
        Self { gray: template_gray, scales, threshold, max_matches }
    }

    /// 在 `frame_gray` 的 `roi` 内搜索。同一位置附近的重复命中会被抑制。
    pub fn find(&self, frame_gray: &core::Mat, roi: core::Rect) -> Result<Vec<NccMatch>> {
        let mut out = Vec::new();
        let search = frame_gray.roi(roi)?.try_clone()?;
        for &s in &self.scales {
            let mut tpl = core::Mat::default();
            if (s - 1.0).abs() < 1e-9 {
                tpl = self.gray.try_clone()?;
            } else {
                imgproc::resize(&self.gray, &mut tpl, core::Size::new(0, 0), s, s, imgproc::INTER_LINEAR)?;
            }
            if tpl.rows() > search.rows() || tpl.cols() > search.cols() {
                continue;
            }
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
                out.push(NccMatch {
                    x: roi.x + max_loc.x,
                    y: roi.y + max_loc.y,
                    w: tpl.cols(),
                    h: tpl.rows(),
                    score: max_val,
                    scale: s,
                });
                // 抑制命中中心附近半个模板大小的区域，避免重复返回同一目标
                let x0 = (max_loc.x - tpl.cols() / 2).max(0);
                let y0 = (max_loc.y - tpl.rows() / 2).max(0);
                let x1 = (max_loc.x + tpl.cols() / 2 + 1).min(rw);
                let y1 = (max_loc.y + tpl.rows() / 2 + 1).min(rh);
                let rect = core::Rect::new(x0, y0, x1 - x0, y1 - y0);
                let mut sub = result.roi_mut(rect)?;
                sub.set_to(&core::Scalar::all(-1.0), &core::no_array())?;
            }
        }
        out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out)
    }
}
