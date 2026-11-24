//! 视觉几何与匹配引擎（OpenCV）。
//!
//! - `ncc`：极速档，`matchTemplate` 多尺度 NCC，带方差检查与非极大值抑制。
//! - `sift`：标准档与精准档，SIFT + ratio test + RANSAC 单应，支持小目标的放大匹配。
//! - 本文件中的几何函数用于校验单应结果（凸性、面积）并计算置信度。

use vf_project::Quad;

pub mod ncc;
pub mod sift;

/// 四边形面积（鞋带公式）。
pub fn quad_area(q: &Quad) -> f64 {
    let mut s: f64 = 0.0;
    for i in 0..4 {
        let j = (i + 1) % 4;
        s += q[i][0] * q[j][1] - q[j][0] * q[i][1];
    }
    s.abs() / 2.0
}

/// 凸性检查：所有相邻边的叉积符号一致且非零。
pub fn is_convex(q: &Quad) -> bool {
    let mut sign = 0.0_f64;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let c = q[(i + 2) % 4];
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() < 1e-9 {
            return false;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// SIFT 匹配的置信度，取值 0–1。由三个因子的几何平均组成：
/// - 内点数：6 个以下几乎不可信，14 个（即 20 个内点）以上满分；
/// - 内点率（内点 / 好的匹配）：低于 25% 为 0，75% 以上满分；
/// - 重投影误差：3 像素以上为 0，0 像素为满分。
///
/// 任一因子为 0 时置信度为 0。参数由合成场景标定（见 `vf-pipeline` 的级联测试）；
/// 用真实素材复核时，需要重新标定（ADR-0007）。
pub fn match_confidence(inliers: usize, good_matches: usize, reproj_error_px: f64) -> f64 {
    if good_matches == 0 || inliers == 0 || !reproj_error_px.is_finite() {
        return 0.0;
    }
    let count = ((inliers as f64 - 6.0) / 14.0).clamp(0.0, 1.0);
    let ratio = ((inliers as f64 / good_matches as f64 - 0.25) / 0.5).clamp(0.0, 1.0);
    let err = (1.0 - reproj_error_px / 3.0).clamp(0.0, 1.0);
    (count * ratio * err).cbrt()
}

/// 合成测试场景：界面风格的平坦背景、控件色块和一个目标。供各 crate 的测试共用。
pub mod testutil {
    use opencv::{core, imgproc, prelude::*};

    /// 目标在源分辨率（1920x1080）中的位置与尺寸。
    pub const OBJ: (i32, i32, i32, i32) = (800, 400, 200, 120);

    pub fn fill(m: &mut core::Mat, r: core::Rect, v: f64) -> opencv::Result<()> {
        m.roi_mut(r)?.set_to(&core::Scalar::all(v), &core::no_array())?;
        Ok(())
    }

    /// 源分辨率的界面场景。`textured` 为真时目标是带细节的随机纹理，否则是平坦背景上的一个深色条。
    pub fn source_scene(textured: bool) -> opencv::Result<core::Mat> {
        core::set_rng_seed(20240601)?;
        let mut src = core::Mat::zeros(1080, 1920, core::CV_8UC1)?.to_mat()?;
        fill(&mut src, core::Rect::new(0, 0, 1920, 1080), 240.0)?;
        fill(&mut src, core::Rect::new(100, 100, 600, 80), 200.0)?;
        fill(&mut src, core::Rect::new(100, 220, 600, 80), 120.0)?;
        fill(&mut src, core::Rect::new(1200, 700, 500, 300), 225.0)?;
        let (x, y, w, h) = OBJ;
        if textured {
            let mut noise = core::Mat::zeros(h, w, core::CV_8UC1)?.to_mat()?;
            core::randu(&mut noise, &core::Scalar::all(0.0), &core::Scalar::all(255.0))?;
            let mut blurred = core::Mat::default();
            imgproc::gaussian_blur(&noise, &mut blurred, core::Size::new(0, 0), 1.0, 1.0, core::BORDER_DEFAULT)?;
            blurred.copy_to(&mut src.roi_mut(core::Rect::new(x, y, w, h))?)?;
        } else {
            fill(&mut src, core::Rect::new(x, y, w, h), 240.0)?;
            fill(&mut src, core::Rect::new(x + 50, y + 30, 100, 60), 60.0)?;
        }
        Ok(src)
    }

    /// 源场景缩放到代理分辨率（960x540，双三次），并叠加压缩噪声与轻微亮度偏移。
    pub fn proxy_frame(src: &core::Mat) -> opencv::Result<core::Mat> {
        core::set_rng_seed(7)?;
        let mut scaled = core::Mat::default();
        imgproc::resize(src, &mut scaled, core::Size::new(960, 540), 0.0, 0.0, imgproc::INTER_CUBIC)?;
        let mut f = core::Mat::default();
        scaled.convert_to(&mut f, core::CV_32FC1, 1.0, 0.0)?;
        let mut noise = core::Mat::zeros(540, 960, core::CV_32FC1)?.to_mat()?;
        core::randn(&mut noise, &core::Scalar::all(0.0), &core::Scalar::all(4.0))?;
        let mut noisy = core::Mat::default();
        core::add(&f, &noise, &mut noisy, &core::no_array(), -1)?;
        let mut out = core::Mat::default();
        noisy.convert_to(&mut out, core::CV_8UC1, 1.0, 5.0)?;
        Ok(out)
    }

    /// 目标的代理分辨率模板（源分辨率裁剪后缩小一半）。
    pub fn proxy_template(src: &core::Mat) -> opencv::Result<core::Mat> {
        let (x, y, w, h) = OBJ;
        let crop = src.roi(core::Rect::new(x, y, w, h))?.try_clone()?;
        let mut out = core::Mat::default();
        imgproc::resize(&crop, &mut out, core::Size::new(0, 0), 0.5, 0.5, imgproc::INTER_AREA)?;
        Ok(out)
    }

    /// 代理分辨率下目标的位置（左上角）与尺寸。
    pub fn proxy_obj() -> (f64, f64, f64, f64) {
        let (x, y, w, h) = OBJ;
        (x as f64 / 2.0, y as f64 / 2.0, w as f64 / 2.0, h as f64 / 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq() -> Quad {
        [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn square_area_and_convexity() {
        assert!((quad_area(&sq()) - 100.0).abs() < 1e-9);
        assert!(is_convex(&sq()));
    }

    #[test]
    fn bow_tie_is_not_convex() {
        let q: Quad = [[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]];
        assert!(!is_convex(&q));
    }

    #[test]
    fn confidence_is_monotonic_and_bounded() {
        assert_eq!(match_confidence(0, 0, 0.0), 0.0);
        assert_eq!(match_confidence(10, 20, f64::INFINITY), 0.0);
        let low = match_confidence(10, 20, 1.0);
        let high = match_confidence(30, 40, 1.0);
        assert!(high > low);
        assert!((0.0..=1.0).contains(&high));
        assert!(match_confidence(30, 30, 0.5) > match_confidence(30, 30, 3.0));
    }

    #[test]
    fn confidence_matches_calibration_points() {
        // 20 个以上内点、几乎无误差、内点率高：接近满分
        assert!(match_confidence(21, 21, 0.13) > 0.95);
        // 十几个内点、内点率一般：贴近常见阈值 0.8 以下
        assert!(match_confidence(12, 30, 1.0) < 0.6);
        // 内点太少：直接为 0
        assert_eq!(match_confidence(5, 6, 0.5), 0.0);
    }
}
