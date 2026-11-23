//! 打码渲染（OpenCV，对应 `first_design.md` 2.7 与 1.4）。
//!
//! 流程：外扩四边形 → 多边形掩膜（`fillPoly`）→ 生成效果图（马赛克 / 模糊 / 色块）→
//! 按掩膜合成，开启羽化时用模糊后的软掩膜做 alpha 混合。
//!
//! 性能约束：
//! - 只处理打码框（外接矩形加必要边距），不对整帧做运算；
//! - 中间缓冲区由 [`RenderScratch`] 预分配并复用，逐帧不再分配整帧矩阵；
//! - 调用方可以把帧缓冲区包装成 `Mat` 后原地修改，不需要拷贝。

use opencv::boxed_ref::{BoxedRef, BoxedRefMut};
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

/// 按四边形光栅化掩膜（单通道，目标区域为 255）。整帧尺寸，仅供测试与工具使用。
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

/// 整数像素矩形（帧坐标）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect2 {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// 四边形在帧内的整数外接矩形；完全在帧外时返回 `None`。
fn bounds(q: &Quad, fw: i32, fh: i32) -> Option<Rect2> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in q {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let x0 = (x0.floor() as i32).clamp(0, fw);
    let y0 = (y0.floor() as i32).clamp(0, fh);
    let x1 = (x1.ceil() as i32).clamp(0, fw);
    let y1 = (y1.ceil() as i32).clamp(0, fh);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect2 { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

/// 向四周扩展 `m` 像素，并截断到帧内。
fn grow(r: Rect2, m: i32, fw: i32, fh: i32) -> Rect2 {
    let x0 = (r.x - m).max(0);
    let y0 = (r.y - m).max(0);
    let x1 = (r.x + r.w + m).min(fw);
    let y1 = (r.y + r.h + m).min(fh);
    Rect2 { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

/// 把矩形向外对齐到 `block` 的整数网格（以帧原点为基准），并截断到帧内。
/// 马赛克按这个网格取块，保证每个块完整地在打码区域内或被整块覆盖。
fn align_to_grid(r: Rect2, block: i32, fw: i32, fh: i32) -> Rect2 {
    let x0 = (r.x / block) * block;
    let y0 = (r.y / block) * block;
    let x1 = (((r.x + r.w + block - 1) / block) * block).min(fw);
    let y1 = (((r.y + r.h + block - 1) / block) * block).min(fh);
    Rect2 { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

/// 预分配的渲染缓冲区。按需增长，之后逐帧复用，不再为每一帧分配整帧矩阵。
#[derive(Default)]
pub struct RenderScratch {
    mask: Option<core::Mat>,
    mask_soft: Option<core::Mat>,
    effect: Option<core::Mat>,
    small: Option<core::Mat>,
    gray_f: Option<core::Mat>,
    orig_f: Option<core::Mat>,
    eff_f: Option<core::Mat>,
    diff_f: Option<core::Mat>,
    alpha_f: Option<core::Mat>,
}

/// 取一个至少 `rows × cols` 的缓冲区，返回左上角 `rows × cols` 的可变视图（不分配新内存，除非需要增长）。
fn view_mut(
    slot: &mut Option<core::Mat>,
    rows: i32,
    cols: i32,
    typ: i32,
) -> Result<BoxedRefMut<'_, core::Mat>, RenderError> {
    let grow = match slot.as_ref() {
        Some(m) => m.rows() < rows || m.cols() < cols || m.typ() != typ,
        None => true,
    };
    if grow {
        let (r0, c0) = slot.as_ref().map_or((0, 0), |m| (m.rows(), m.cols()));
        let r = rows.max(r0);
        let c = cols.max(c0);
        *slot = Some(core::Mat::zeros(r, c, typ)?.to_mat()?);
    }
    let m = slot.as_mut().expect("刚刚分配过");
    Ok(m.roi_mut(core::Rect::new(0, 0, cols, rows))?)
}

/// 只读取缓冲区左上角 `rows × cols` 的视图。调用前缓冲区必须已经分配。
fn view(slot: &Option<core::Mat>, rows: i32, cols: i32) -> Result<BoxedRef<'_, core::Mat>, RenderError> {
    let m = slot.as_ref().expect("缓冲区已分配");
    Ok(m.roi(core::Rect::new(0, 0, cols, rows))?)
}

/// 对 `frame`（CV_8UC3）中的四边形区域打码，结果原地写回。返回是否实际作用于了画面。
///
/// `scratch` 应在同一线程内跨帧复用；它只包含缓冲区，不含帧数据。
pub fn render_region_in(
    scratch: &mut RenderScratch,
    frame: &mut impl MatTrait,
    quad: &Quad,
    style: &Style,
    speed_px_per_frame: f64,
) -> Result<bool, RenderError> {
    if style.kind == MaskKind::Image {
        return Err(RenderError::Unsupported(MaskKind::Image));
    }
    let (fw, fh) = (frame.cols(), frame.rows());
    let pad = if style.motion_pad {
        motion_padding(style.padding as f64, speed_px_per_frame)
    } else {
        style.padding as f64
    };
    let area = expand_quad(quad, pad);
    let Some(bb) = bounds(&area, fw, fh) else { return Ok(false) };

    // 边距：模糊核半径约为 3 个 sigma；羽化的软掩膜核半径为 4。
    let sigma = blur_sigma(style.strength);
    let feather_margin = if style.feather { 6 } else { 0 };
    let blur_margin = if style.kind == MaskKind::Blur { (sigma * 3.0).ceil() as i32 + 2 } else { 0 };
    let mut roi = grow(bb, blur_margin.max(feather_margin), fw, fh);
    if style.kind == MaskKind::Mosaic {
        roi = align_to_grid(roi, mosaic_block(style.strength), fw, fh);
    }
    let (w, h) = (roi.w, roi.h);
    let roi_rect = core::Rect::new(roi.x, roi.y, w, h);

    // 1. 掩膜（ROI 坐标系）
    {
        let mut mask = view_mut(&mut scratch.mask, h, w, core::CV_8UC1)?;
        mask.set_to(&core::Scalar::all(0.0), &core::no_array())?;
        let pts: core::Vector<core::Point> = area
            .iter()
            .map(|p| core::Point::new(p[0].round() as i32 - roi.x, p[1].round() as i32 - roi.y))
            .collect();
        let polys = core::Vector::<core::Vector<core::Point>>::from_iter([pts]);
        imgproc::fill_poly(&mut mask, &polys, core::Scalar::all(255.0), imgproc::LINE_8, 0, core::Point::default())?;
    }

    // 2. 效果图（ROI 坐标系）
    match style.kind {
        MaskKind::Solid => {
            let mut e = view_mut(&mut scratch.effect, h, w, core::CV_8UC3)?;
            e.set_to(&core::Scalar::all(0.0), &core::no_array())?;
        }
        MaskKind::Blur => {
            let k = ((sigma * 6.0).round() as i32).max(3) | 1;
            let src = frame.roi(roi_rect)?;
            let mut e = view_mut(&mut scratch.effect, h, w, core::CV_8UC3)?;
            imgproc::gaussian_blur(&src, &mut e, core::Size::new(k, k), sigma, sigma, core::BORDER_DEFAULT)?;
        }
        MaskKind::Mosaic => {
            // 块以帧原点为基准对齐（见 align_to_grid），每块取平均后整块填充。
            let block = mosaic_block(style.strength);
            let sw = (w + block - 1) / block;
            let sh = (h + block - 1) / block;
            {
                let src = frame.roi(roi_rect)?;
                let mut small = view_mut(&mut scratch.small, sh, sw, core::CV_8UC3)?;
                imgproc::resize(&src, &mut small, core::Size::new(sw, sh), 0.0, 0.0, imgproc::INTER_AREA)?;
            }
            let small = view(&scratch.small, sh, sw)?;
            let mut e = view_mut(&mut scratch.effect, h, w, core::CV_8UC3)?;
            imgproc::resize(&small, &mut e, core::Size::new(w, h), 0.0, 0.0, imgproc::INTER_NEAREST)?;
        }
        MaskKind::Image => return Err(RenderError::Unsupported(MaskKind::Image)),
    }

    // 3. 合成
    if style.feather {
        // 软掩膜 = 模糊后的掩膜；alpha = 软掩膜 / 255
        {
            let mask = view(&scratch.mask, h, w)?;
            let mut soft = view_mut(&mut scratch.mask_soft, h, w, core::CV_8UC1)?;
            imgproc::gaussian_blur(&mask, &mut soft, core::Size::new(9, 9), 3.0, 3.0, core::BORDER_DEFAULT)?;
        }
        {
            let soft = view(&scratch.mask_soft, h, w)?;
            let mut gray = view_mut(&mut scratch.gray_f, h, w, core::CV_32FC1)?;
            soft.convert_to(&mut gray, core::CV_32F, 1.0 / 255.0, 0.0)?;
        }
        {
            let gray = view(&scratch.gray_f, h, w)?;
            let mut alpha = view_mut(&mut scratch.alpha_f, h, w, core::CV_32FC3)?;
            // OpenCV 4.11 起 cvtColor 多一个 AlgorithmHint 参数，见 build/opencv_version.rs。
            #[cfg(opencv_ge411)]
            imgproc::cvt_color(&gray, &mut alpha, imgproc::COLOR_GRAY2RGB, 0, core::AlgorithmHint::ALGO_HINT_DEFAULT)?;
            #[cfg(not(opencv_ge411))]
            imgproc::cvt_color(&gray, &mut alpha, imgproc::COLOR_GRAY2RGB, 0)?;
        }
        // 结果 = 原图 + (效果图 - 原图) × alpha
        {
            let orig_src = frame.roi(roi_rect)?;
            let mut orig = view_mut(&mut scratch.orig_f, h, w, core::CV_32FC3)?;
            orig_src.convert_to(&mut orig, core::CV_32FC3, 1.0, 0.0)?;
        }
        {
            let eff_src = view(&scratch.effect, h, w)?;
            let mut eff = view_mut(&mut scratch.eff_f, h, w, core::CV_32FC3)?;
            eff_src.convert_to(&mut eff, core::CV_32FC3, 1.0, 0.0)?;
        }
        {
            let orig = view(&scratch.orig_f, h, w)?;
            let eff = view(&scratch.eff_f, h, w)?;
            let mut diff = view_mut(&mut scratch.diff_f, h, w, core::CV_32FC3)?;
            core::subtract(&eff, &orig, &mut diff, &core::no_array(), -1)?;
        }
        {
            let alpha = view(&scratch.alpha_f, h, w)?;
            let diff = view(&scratch.diff_f, h, w)?;
            let mut prod = view_mut(&mut scratch.eff_f, h, w, core::CV_32FC3)?;
            core::multiply(&diff, &alpha, &mut prod, 1.0, -1)?;
        }
        {
            let orig = view(&scratch.orig_f, h, w)?;
            let prod = view(&scratch.eff_f, h, w)?;
            let mut out = view_mut(&mut scratch.diff_f, h, w, core::CV_32FC3)?;
            core::add(&orig, &prod, &mut out, &core::no_array(), -1)?;
        }
        let out = view(&scratch.diff_f, h, w)?;
        let mut dst = frame.roi_mut(roi_rect)?;
        out.convert_to(&mut dst, core::CV_8UC3, 1.0, 0.0)?;
    } else {
        let mask = view(&scratch.mask, h, w)?;
        let effect = view(&scratch.effect, h, w)?;
        let mut dst = frame.roi_mut(roi_rect)?;
        effect.copy_to_masked(&mut dst, &mask)?;
    }
    Ok(true)
}

/// 对 `frame` 的四边形区域打码（便捷接口，每次调用都会新建缓冲区；逐帧处理请使用 [`render_region_in`]）。
pub fn render_region(
    frame: &mut impl MatTrait,
    quad: &Quad,
    style: &Style,
    speed_px_per_frame: f64,
) -> Result<(), RenderError> {
    let mut scratch = RenderScratch::default();
    render_region_in(&mut scratch, frame, quad, style, speed_px_per_frame).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::core::Vec3b;

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
        let mut m = core::Mat::zeros(16, 16, core::CV_8UC3).unwrap().to_mat().unwrap();
        m.data_bytes_mut().unwrap().copy_from_slice(&data);
        m
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
        assert_eq!(*m.at_2d::<Vec3b>(0, 0).unwrap(), *before.at_2d::<Vec3b>(0, 0).unwrap());
        assert_eq!(*m.at_2d::<Vec3b>(5, 5).unwrap(), Vec3b::from([0, 0, 0]));
    }

    #[test]
    fn mosaic_makes_blocks_uniform() {
        let mut m = gradient();
        let mut s = style(MaskKind::Mosaic);
        s.strength = 0.0; // 块边长 4
        render_region(&mut m, &square(0.0, 0.0, 16.0), &s, 0.0).unwrap();
        let a = *m.at_2d::<Vec3b>(0, 0).unwrap();
        assert_eq!(*m.at_2d::<Vec3b>(3, 3).unwrap(), a);
        assert_ne!(*m.at_2d::<Vec3b>(0, 4).unwrap(), a);
    }

    #[test]
    fn blur_reduces_contrast_inside_region() {
        let mut data = vec![0u8; 16 * 16 * 3];
        let i = (8 * 16 + 8) * 3;
        data[i..i + 3].copy_from_slice(&[255, 255, 255]);
        let mut m = core::Mat::zeros(16, 16, core::CV_8UC3).unwrap().to_mat().unwrap();
        m.data_bytes_mut().unwrap().copy_from_slice(&data);
        render_region(&mut m, &square(0.0, 0.0, 16.0), &style(MaskKind::Blur), 0.0).unwrap();
        assert!(m.at_2d::<Vec3b>(8, 8).unwrap()[0] < 255);
    }

    #[test]
    fn feather_softens_edge_without_touching_far_pixels() {
        let mut m = gradient();
        let before = m.try_clone().unwrap();
        let mut s = style(MaskKind::Solid);
        s.feather = true;
        render_region(&mut m, &square(4.0, 4.0, 4.0), &s, 0.0).unwrap();
        assert_eq!(*m.at_2d::<Vec3b>(15, 15).unwrap(), *before.at_2d::<Vec3b>(15, 15).unwrap());
        // 小方块经过高斯软化后中心 alpha 小于 1，只要求中心比原图变暗
        let center = *m.at_2d::<Vec3b>(5, 5).unwrap();
        let original = *before.at_2d::<Vec3b>(5, 5).unwrap();
        assert!(center[0] < original[0], "center should be darker: {center:?} vs {original:?}");
    }

    #[test]
    fn image_kind_is_unsupported() {
        let mut m = gradient();
        let err = render_region(&mut m, &square(0.0, 0.0, 4.0), &style(MaskKind::Image), 0.0);
        assert!(matches!(err, Err(RenderError::Unsupported(MaskKind::Image))));
    }

    #[test]
    fn scratch_reuse_gives_same_result_as_fresh_buffers() {
        let quad = square(3.0, 2.0, 7.0);
        let mut s = style(MaskKind::Blur);
        s.feather = true;
        let mut fresh = gradient();
        render_region(&mut fresh, &quad, &s, 0.0).unwrap();

        let mut scratch = RenderScratch::default();
        let mut reused = gradient();
        // 先用别的参数处理一次，确保缓冲区里残留的数据不会影响结果
        render_region_in(&mut scratch, &mut reused, &square(0.0, 0.0, 16.0), &style(MaskKind::Solid), 0.0).unwrap();
        let mut again = gradient();
        render_region_in(&mut scratch, &mut again, &quad, &s, 0.0).unwrap();
        assert_eq!(fresh.data_bytes().unwrap(), again.data_bytes().unwrap());
    }

    #[test]
    fn expand_and_motion_padding() {
        let e = expand_quad(&square(10.0, 10.0, 10.0), 2.0);
        assert!(e[0][0] < 10.0 && e[2][0] > 20.0);
        assert_eq!(motion_padding(4.0, 0.0), 4.0);
        assert_eq!(motion_padding(4.0, 100.0), 20.0);
    }
}
