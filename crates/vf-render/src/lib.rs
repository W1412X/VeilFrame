//! 打码渲染（OpenCV，对应 `first_design.md` 2.7 与 1.4）。
//!
//! 流程：外扩四边形 → 多边形掩膜（`fillPoly`）→ 生成效果图（马赛克 / 模糊 / 色块）→
//! 按掩膜合成，开启羽化时用模糊后的软掩膜做 alpha 混合。

use opencv::{core, imgproc, prelude::*};
use vf_project::{MaskKind, Quad, Style};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("打码类型 {0:?} 尚未实现")]
    Unsupported(MaskKind),
    #[error("OpenCV 错误：{0}")]
    Cv(String),
}

impl From<opencv::Error> for RenderError {
    fn from(e: opencv::Error) -> Self {
        RenderError::Cv(e.to_string())
    }
}

/// 从质心沿径向外扩 `pad` 像素（对凸四边形是近似外扩）。
pub fn expand_quad(q: &Quad, pad: f64) -> Quad {
    let cx = q.iter().map(|p| p[0]).sum::<f64>() / 4.0;
    let cy = q.iter().map(|p| p[1]).sum::<f64>() / 4.0;
    let mut out = *q;
    for p in out.iter_mut() {
        let (vx, vy) = (p[0] - cx, p[1] - cy);
        let len = (vx * vx + vy * vy).sqrt();
        if len > 1e-9 {
            p[0] += vx / len * pad;
            p[1] += vy / len * pad;
        }
    }
    out
}

/// 运动自适应外扩：目标移动越快，外扩越大，上限 16 px。
pub fn motion_padding(base_px: f64, speed_px_per_frame: f64) -> f64 {
    base_px + (speed_px_per_frame * 2.0).min(16.0)
}

/// 强度 → 马赛克块边长（像素）。
pub fn mosaic_block(strength: f64) -> i32 {
    4 + (strength.clamp(0.0, 1.0) * 20.0).round() as i32
}

/// 强度 → 高斯模糊 sigma。
pub fn blur_sigma(strength: f64) -> f64 {
    1.0 + strength.clamp(0.0, 1.0) * 8.0
}

/// 按四边形光栅化掩膜（单通道，目标区域为 255）。
pub fn polygon_mask(q: &Quad, rows: i32, cols: i32) -> Result<core::Mat, RenderError> {
    let mut mask = core::Mat::zeros(rows, cols, core::CV_8UC1)?.to_mat()?;
    let pts: core::Vector<core::Point> = q
        .iter()
        .map(|p| core::Point::new(p[0].round() as i32, p[1].round() as i32))
        .collect();
    let polys = core::Vector::<core::Vector<core::Point>>::from_iter([pts]);
    imgproc::fill_poly(&mut mask, &polys, core::Scalar::all(255.0), imgproc::LINE_8, 0, core::Point::default())?;
    Ok(mask)
}

/// 对 `frame`（CV_8UC3，RGB）中的四边形区域打码，结果原地写回。
pub fn render_region(frame: &mut core::Mat, quad: &Quad, style: &Style, speed_px_per_frame: f64) -> Result<(), RenderError> {
    if style.kind == MaskKind::Image {
        return Err(RenderError::Unsupported(MaskKind::Image));
    }
    let pad = if style.motion_pad {
        motion_padding(style.padding as f64, speed_px_per_frame)
    } else {
        style.padding as f64
    };
    let area = expand_quad(quad, pad);
    let size = frame.size()?;
    let mask = polygon_mask(&area, size.height, size.width)?;

    let effect = match style.kind {
        MaskKind::Mosaic => {
            let block = mosaic_block(style.strength);
            let small_size = core::Size::new((size.width / block).max(1), (size.height / block).max(1));
            let mut small = core::Mat::default();
            imgproc::resize(frame, &mut small, small_size, 0.0, 0.0, imgproc::INTER_AREA)?;
            let mut up = core::Mat::default();
            imgproc::resize(&small, &mut up, size, 0.0, 0.0, imgproc::INTER_NEAREST)?;
            up
        }
        MaskKind::Blur => {
            let sigma = blur_sigma(style.strength);
            let k = ((sigma * 6.0).round() as i32).max(3) | 1;
            let mut blurred = core::Mat::default();
            imgproc::gaussian_blur(frame, &mut blurred, core::Size::new(k, k), sigma, sigma, core::BORDER_DEFAULT)?;
            blurred
        }
        MaskKind::Solid => core::Mat::zeros(size.height, size.width, frame.typ())?.to_mat()?,
        MaskKind::Image => return Err(RenderError::Unsupported(MaskKind::Image)),
    };

    if style.feather {
        blend_feathered(frame, &effect, &mask)?;
    } else {
        effect.copy_to_masked(frame, &mask)?;
    }
    Ok(())
}

/// 羽化合成：软掩膜（高斯模糊后的掩膜）作为 alpha，结果 = 原图 + (效果图 - 原图) × alpha。
fn blend_feathered(frame: &mut core::Mat, effect: &core::Mat, mask: &core::Mat) -> Result<(), RenderError> {
    let mut soft = core::Mat::default();
    imgproc::gaussian_blur(mask, &mut soft, core::Size::new(9, 9), 3.0, 3.0, core::BORDER_DEFAULT)?;
    let mut alpha32 = core::Mat::default();
    soft.convert_to(&mut alpha32, core::CV_32F, 1.0 / 255.0, 0.0)?;
    let mut alpha3 = core::Mat::default();
    // OpenCV 4.11 起 cvtColor 多一个 AlgorithmHint 参数，见 build/opencv_version.rs。
    #[cfg(opencv_ge411)]
    imgproc::cvt_color(
        &alpha32,
        &mut alpha3,
        imgproc::COLOR_GRAY2RGB,
        0,
        core::AlgorithmHint::ALGO_HINT_DEFAULT,
    )?;
    #[cfg(not(opencv_ge411))]
    imgproc::cvt_color(&alpha32, &mut alpha3, imgproc::COLOR_GRAY2RGB, 0)?;

    let mut orig = core::Mat::default();
    frame.convert_to(&mut orig, core::CV_32FC3, 1.0, 0.0)?;
    let mut eff = core::Mat::default();
    effect.convert_to(&mut eff, core::CV_32FC3, 1.0, 0.0)?;

    let mut diff = core::Mat::default();
    core::subtract(&eff, &orig, &mut diff, &core::no_array(), -1)?;
    let mut weighted = core::Mat::default();
    core::multiply(&diff, &alpha3, &mut weighted, 1.0, -1)?;
    let mut out = core::Mat::default();
    core::add(&orig, &weighted, &mut out, &core::no_array(), -1)?;

    let mut out8 = core::Mat::default();
    out.convert_to(&mut out8, core::CV_8UC3, 1.0, 0.0)?;
    out8.copy_to(frame)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_image::mat_from_rgb;

    fn square(x0: f64, y0: f64, size: f64) -> Quad {
        [[x0, y0], [x0 + size, y0], [x0 + size, y0 + size], [x0, y0 + size]]
    }

    fn style(kind: MaskKind) -> Style {
        Style { kind, strength: 0.5, padding: 0, feather: false, motion_pad: false }
    }

    /// 16x16 的渐变图（R 随 x 变化），便于检测效果
    fn gradient() -> core::Mat {
        let mut data = Vec::new();
        for _y in 0..16 {
            for x in 0..16 {
                data.extend_from_slice(&[(x * 16) as u8, 40, 200]);
            }
        }
        mat_from_rgb(16, 16, &data).unwrap()
    }

    #[test]
    fn polygon_mask_covers_square_area() {
        let m = polygon_mask(&square(2.0, 3.0, 10.0), 32, 32).unwrap();
        let count = core::count_non_zero(&m).unwrap();
        assert!((count - 100).abs() <= 21, "count was {count}");
    }

    #[test]
    fn solid_only_changes_masked_pixels() {
        let mut m = gradient();
        let before = m.try_clone().unwrap();
        render_region(&mut m, &square(4.0, 4.0, 4.0), &style(MaskKind::Solid), 0.0).unwrap();
        assert_eq!(*m.at_2d::<core::Vec3b>(0, 0).unwrap(), *before.at_2d::<core::Vec3b>(0, 0).unwrap());
        assert_eq!(*m.at_2d::<core::Vec3b>(5, 5).unwrap(), core::Vec3b::from([0, 0, 0]));
    }

    #[test]
    fn mosaic_makes_blocks_uniform() {
        let mut m = gradient();
        let mut s = style(MaskKind::Mosaic);
        s.strength = 0.0; // 块边长 4
        render_region(&mut m, &square(0.0, 0.0, 16.0), &s, 0.0).unwrap();
        let a = *m.at_2d::<core::Vec3b>(0, 0).unwrap();
        assert_eq!(*m.at_2d::<core::Vec3b>(3, 3).unwrap(), a);
        assert_ne!(*m.at_2d::<core::Vec3b>(0, 4).unwrap(), a);
    }

    #[test]
    fn blur_reduces_contrast_inside_region() {
        let mut data = vec![0u8; 16 * 16 * 3];
        let i = (8 * 16 + 8) * 3;
        data[i..i + 3].copy_from_slice(&[255, 255, 255]);
        let mut m = mat_from_rgb(16, 16, &data).unwrap();
        render_region(&mut m, &square(0.0, 0.0, 16.0), &style(MaskKind::Blur), 0.0).unwrap();
        assert!(m.at_2d::<core::Vec3b>(8, 8).unwrap()[0] < 255);
    }

    #[test]
    fn feather_softens_edge_without_touching_far_pixels() {
        let mut m = gradient();
        let before = m.try_clone().unwrap();
        let mut s = style(MaskKind::Solid);
        s.feather = true;
        render_region(&mut m, &square(4.0, 4.0, 4.0), &s, 0.0).unwrap();
        assert_eq!(*m.at_2d::<core::Vec3b>(15, 15).unwrap(), *before.at_2d::<core::Vec3b>(15, 15).unwrap());
        // 小方块经过高斯软化后中心 alpha 小于 1，只要求中心比原图变暗
        let center = *m.at_2d::<core::Vec3b>(5, 5).unwrap();
        let original = *before.at_2d::<core::Vec3b>(5, 5).unwrap();
        assert!(center[0] < original[0], "center should be darker: {center:?} vs {original:?}");
    }

    #[test]
    fn image_kind_is_unsupported() {
        let mut m = gradient();
        let err = render_region(&mut m, &square(0.0, 0.0, 4.0), &style(MaskKind::Image), 0.0);
        assert!(matches!(err, Err(RenderError::Unsupported(MaskKind::Image))));
    }

    #[test]
    fn expand_and_motion_padding() {
        let e = expand_quad(&square(10.0, 10.0, 10.0), 2.0);
        assert!(e[0][0] < 10.0 && e[2][0] > 20.0);
        assert_eq!(motion_padding(4.0, 0.0), 4.0);
        assert_eq!(motion_padding(4.0, 100.0), 20.0);
    }
}
