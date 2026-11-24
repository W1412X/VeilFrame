//! 标准档与精准档：SIFT 特征 + Lowe ratio test + RANSAC 单应（`first_design.md` 2.3）。
//!
//! 坐标约定：模板与帧都由调用方给出（代理分辨率；精准档为源分辨率）。
//! `scale > 1` 时，匹配器在提取特征前把输入放大 `scale` 倍，用于小目标；
//! 返回的四边形已经换算回输入的坐标系。模板和帧使用同一个 `scale`，保证尺度一致。
//! 输入先做 CLAHE 对比度均衡，弱对比度的界面元素也能提取到特征点。

use opencv::imgproc::CLAHETrait;
use opencv::{calib3d, core, features2d, features2d::DescriptorMatcherTrait, features2d::Feature2DTrait, imgproc, prelude::*, Result};
use vf_project::Quad;

use crate::{is_convex, match_confidence, quad_area};

const CLAHE_CLIP: f64 = 2.0;
const LOWE_RATIO: f32 = 0.75;
const RANSAC_THRESHOLD_PX: f64 = 4.0;
/// 好的匹配数低于此值时不做单应估计。
pub const MIN_GOOD_MATCHES: usize = 8;
const MIN_INLIERS: usize = 8;
const MIN_INLIER_RATIO: f64 = 0.3;
const MIN_QUAD_AREA: f64 = 16.0;

/// 一次定位的结果。`quad` 为帧输入坐标系下的四边形。
#[derive(Debug, Clone, PartialEq)]
pub struct SiftLocated {
    pub quad: Quad,
    pub inliers: usize,
    pub good: usize,
    pub reproj_error_px: f64,
    pub conf: f64,
}

fn enhance(gray: &core::Mat) -> Result<core::Mat> {
    let mut out = core::Mat::default();
    let mut clahe = imgproc::create_clahe(CLAHE_CLIP, core::Size::new(8, 8))?;
    clahe.apply(gray, &mut out)?;
    Ok(out)
}

fn upscale(src: &core::Mat, s: f64) -> Result<core::Mat> {
    if (s - 1.0).abs() < 1e-9 {
        return src.try_clone();
    }
    let mut out = core::Mat::default();
    imgproc::resize(src, &mut out, core::Size::new(0, 0), s, s, imgproc::INTER_CUBIC)?;
    Ok(out)
}

pub struct SiftMatcher {
    scale: f64,
    sift: core::Ptr<features2d::SIFT>,
    /// 模板描述子作为训练集存放在匹配器中，每次匹配时帧的描述子作为查询。
    matcher: features2d::BFMatcher,
    tpl_kp: core::Vector<core::KeyPoint>,
    tpl_desc: core::Mat,
    /// 模板在输入坐标系下的宽高（未放大前）。
    tpl_w: f32,
    tpl_h: f32,
}

impl SiftMatcher {
    /// `template_gray` 为单通道模板，已经是帧所在的分辨率。`scale` 为特征提取时的放大倍数（≥1）。
    pub fn new(template_gray: &core::Mat, scale: f64) -> Result<Self> {
        let scale = scale.max(1.0);
        let up = enhance(&upscale(template_gray, scale)?)?;
        let mut sift = features2d::SIFT::create_def()?;
        let mut tpl_kp = core::Vector::<core::KeyPoint>::new();
        let mut tpl_desc = core::Mat::default();
        sift.detect_and_compute_def(&up, &core::no_array(), &mut tpl_kp, &mut tpl_desc)?;
        let mut matcher = features2d::BFMatcher::new(core::NORM_L2, false)?;
        if !tpl_desc.empty() {
            matcher.add(&tpl_desc)?;
        }
        Ok(Self {
            scale,
            sift,
            matcher,
            tpl_kp,
            tpl_desc,
            tpl_w: template_gray.cols() as f32,
            tpl_h: template_gray.rows() as f32,
        })
    }

    /// 放大倍数。
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// 模板提取到的特征点数（已按放大倍数计）。用于判断模板是否有足够的纹理。
    pub fn template_keypoints(&self) -> usize {
        self.tpl_kp.len()
    }

    /// 在整帧上定位模板。
    pub fn locate(&mut self, frame_gray: &core::Mat) -> Result<Option<SiftLocated>> {
        let full = core::Rect::new(0, 0, frame_gray.cols(), frame_gray.rows());
        self.locate_roi(frame_gray, full)
    }

    /// 只在 `rect` 区域内定位模板。`rect` 为帧坐标，返回的四边形同样是帧坐标。
    /// 找不到，或几何上不合理（非凸、面积过小）时返回 `None`。
    pub fn locate_roi(&mut self, frame_gray: &core::Mat, rect: core::Rect) -> Result<Option<SiftLocated>> {
        let crop = frame_gray.roi(rect)?.try_clone()?;
        let input = enhance(&upscale(&crop, self.scale)?)?;
        let Some((mut quad, inliers, good, reproj_up)) = self.match_input(&input)? else {
            return Ok(None);
        };
        // 放大坐标 → 裁剪坐标 → 帧坐标
        let inv = 1.0 / self.scale;
        for p in quad.iter_mut() {
            p[0] = p[0] * inv + rect.x as f64;
            p[1] = p[1] * inv + rect.y as f64;
        }
        if !is_convex(&quad) || quad_area(&quad) < MIN_QUAD_AREA {
            return Ok(None);
        }
        let reproj = reproj_up * inv;
        let conf = match_confidence(inliers, good, reproj);
        Ok(Some(SiftLocated { quad, inliers, good, reproj_error_px: reproj, conf }))
    }

    /// 在已放大的输入上匹配。返回（输入坐标系的四边形、内点数、好的匹配数、重投影误差（放大后的像素））。
    fn match_input(&mut self, input: &core::Mat) -> Result<Option<(Quad, usize, usize, f64)>> {
        if self.tpl_desc.empty() {
            return Ok(None);
        }
        let mut kp = core::Vector::<core::KeyPoint>::new();
        let mut desc = core::Mat::default();
        self.sift.detect_and_compute_def(input, &core::no_array(), &mut kp, &mut desc)?;
        if desc.empty() || kp.len() < MIN_GOOD_MATCHES {
            return Ok(None);
        }

        let mut knn = core::Vector::<core::Vector<core::DMatch>>::new();
        self.matcher.knn_match(&desc, &mut knn, 2, &core::no_array(), false)?;

        // 查询是帧特征（query_idx），训练集是模板特征（train_idx）。
        // 模板特征位于放大后的模板上，除以 scale 回到模板的输入坐标系；帧特征位于放大后的输入上。
        let s = self.scale as f32;
        let mut src_pts = core::Vector::<core::Point2f>::new();
        let mut dst_pts = core::Vector::<core::Point2f>::new();
        for pair in knn.iter() {
            if pair.len() < 2 {
                continue;
            }
            let (m, n) = (pair.get(0)?, pair.get(1)?);
            if m.distance < LOWE_RATIO * n.distance {
                let tp = self.tpl_kp.get(m.train_idx as usize)?.pt();
                let fp = kp.get(m.query_idx as usize)?.pt();
                src_pts.push(core::Point2f::new(tp.x / s, tp.y / s));
                dst_pts.push(fp);
            }
        }
        let good = src_pts.len();
        if good < MIN_GOOD_MATCHES {
            return Ok(None);
        }

        let mut mask = core::Mat::default();
        let h = calib3d::find_homography(&src_pts, &dst_pts, &mut mask, calib3d::RANSAC, RANSAC_THRESHOLD_PX)?;
        if h.empty() {
            return Ok(None);
        }
        let inliers = core::count_non_zero(&mask)? as usize;
        if inliers < MIN_INLIERS || (inliers as f64) < MIN_INLIER_RATIO * good as f64 {
            return Ok(None);
        }

        // 模板四角（输入坐标系）经单应映射到放大后的帧坐标
        let corners = core::Vector::<core::Point2f>::from_iter([
            core::Point2f::new(0.0, 0.0),
            core::Point2f::new(self.tpl_w, 0.0),
            core::Point2f::new(self.tpl_w, self.tpl_h),
            core::Point2f::new(0.0, self.tpl_h),
        ]);
        let mut mapped = core::Vector::<core::Point2f>::new();
        core::perspective_transform(&corners, &mut mapped, &h)?;
        let mut quad: Quad = [[0.0f64; 2]; 4];
        for (i, q) in quad.iter_mut().enumerate() {
            let p = mapped.get(i)?;
            *q = [p.x as f64, p.y as f64];
        }

        // 重投影误差：内点在模板上的点经 H 映射后，与对应帧特征点的平均距离
        let mut err_sum = 0.0f64;
        let mut err_n = 0usize;
        for i in 0..src_pts.len() {
            if *mask.at::<u8>(i as i32)? == 0 {
                continue;
            }
            let src = src_pts.get(i)?;
            let dst = dst_pts.get(i)?;
            let one = core::Vector::<core::Point2f>::from_iter([src]);
            let mut out = core::Vector::<core::Point2f>::new();
            core::perspective_transform(&one, &mut out, &h)?;
            let p = out.get(0)?;
            err_sum += (((p.x - dst.x).powi(2) + (p.y - dst.y).powi(2)) as f64).sqrt();
            err_n += 1;
        }
        let reproj = if err_n > 0 { err_sum / err_n as f64 } else { f64::INFINITY };
        Ok(Some((quad, inliers, good, reproj)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{proxy_frame, proxy_obj, proxy_template, source_scene};

    fn near_obj(q: &Quad, tol: f64) -> bool {
        let (ox, oy, _, _) = proxy_obj();
        (q[0][0] - ox).abs() < tol && (q[0][1] - oy).abs() < tol
    }

    #[test]
    fn locates_textured_object_at_proxy_scale() -> opencv::Result<()> {
        let src = source_scene(true)?;
        let frame = proxy_frame(&src)?;
        let tpl = proxy_template(&src)?;
        let mut m = SiftMatcher::new(&tpl, 1.0)?;
        let hit = m.locate(&frame)?.expect("应定位到目标");
        assert!(near_obj(&hit.quad, 6.0), "四边形 {:?}", hit.quad);
        assert!(hit.conf >= 0.8, "置信度 {}", hit.conf);
        Ok(())
    }

    #[test]
    fn upscaled_matcher_finds_object_inside_candidate_region() -> opencv::Result<()> {
        let src = source_scene(true)?;
        let frame = proxy_frame(&src)?;
        let tpl = proxy_template(&src)?;
        let mut m = SiftMatcher::new(&tpl, 2.0)?;
        // 候选区域：目标周围留出一圈余量
        let rect = core::Rect::new(330, 130, 240, 180);
        let hit = m.locate_roi(&frame, rect)?.expect("候选区域内应定位到目标");
        assert!(near_obj(&hit.quad, 6.0), "四边形 {:?}", hit.quad);
        Ok(())
    }

    #[test]
    fn flat_object_gives_no_confident_match() -> opencv::Result<()> {
        let src = source_scene(false)?;
        let frame = proxy_frame(&src)?;
        // 平坦目标（深色条）几乎没有特征点，不应凭空得到高置信结果
        let tpl = proxy_template(&src)?;
        let mut m = SiftMatcher::new(&tpl, 1.0)?;
        let r = m.locate(&frame)?;
        assert!(r.map_or(true, |h| h.conf < 0.8), "平坦目标不应高置信命中");
        Ok(())
    }
}
