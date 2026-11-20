//! 标准档：SIFT 特征 + Lowe ratio test + RANSAC 单应（对应 `first_design.md` 2.3）。
//!
//! 模板特征一次性提取并缓存；每帧只提取帧特征，与模板做 KNN 匹配，
//! 经 ratio test 后用 RANSAC 估计单应矩阵 H，把模板四角映射为帧内四边形。

use opencv::{calib3d, core, features2d, features2d::Feature2DTrait, features2d::DescriptorMatcherTrait, imgproc, prelude::*, Result};
use vf_project::Quad;

use crate::{is_convex, match_confidence, quad_area};

/// 上采样倍数：小 logo 特征点不足时放大模板（文档 2.3 的鲁棒性工程）。
const TEMPLATE_UPSCALE: f64 = 2.0;
const LOWE_RATIO: f32 = 0.75;
const RANSAC_THRESHOLD_PX: f64 = 4.0;
const MIN_GOOD_MATCHES: usize = 10;
const MIN_INLIERS: usize = 8;
const MIN_INLIER_RATIO: f64 = 0.3;

#[derive(Debug, Clone, PartialEq)]
pub struct SiftLocated {
    pub quad: Quad,
    pub inliers: usize,
    pub good: usize,
    pub reproj_error_px: f64,
    pub conf: f64,
}

pub struct SiftMatcher {
    sift: core::Ptr<features2d::SIFT>,
    /// 模板描述子作为训练集存放在匹配器中，每帧的描述子作为查询。
    matcher: features2d::BFMatcher,
    tpl_kp: core::Vector<core::KeyPoint>,
    tpl_desc: core::Mat,
    tpl_w: f32,
    tpl_h: f32,
}

impl SiftMatcher {
    /// `template_gray` 为单通道模板图。
    pub fn new(template_gray: &core::Mat) -> Result<Self> {
        let mut up = core::Mat::default();
        imgproc::resize(
            template_gray,
            &mut up,
            core::Size::new(0, 0),
            TEMPLATE_UPSCALE,
            TEMPLATE_UPSCALE,
            imgproc::INTER_CUBIC,
        )?;
        let mut sift = features2d::SIFT::create_def()?;
        let mut tpl_kp = core::Vector::<core::KeyPoint>::new();
        let mut tpl_desc = core::Mat::default();
        sift.detect_and_compute_def(&up, &core::no_array(), &mut tpl_kp, &mut tpl_desc)?;
        let mut matcher = features2d::BFMatcher::new(core::NORM_L2, false)?;
        matcher.add(&tpl_desc)?;
        Ok(Self {
            sift,
            matcher,
            tpl_kp,
            tpl_desc,
            tpl_w: template_gray.cols() as f32,
            tpl_h: template_gray.rows() as f32,
        })
    }

    /// 在整帧（或 `roi`）上定位模板。找不到或几何不合理时返回 `None`。
    pub fn locate(&mut self, frame_gray: &core::Mat) -> Result<Option<SiftLocated>> {
        let mut kp = core::Vector::<core::KeyPoint>::new();
        let mut desc = core::Mat::default();
        self.sift.detect_and_compute_def(frame_gray, &core::no_array(), &mut kp, &mut desc)?;
        if desc.empty() || self.tpl_desc.empty() || kp.len() < MIN_GOOD_MATCHES {
            return Ok(None);
        }

        let mut knn = core::Vector::<core::Vector<core::DMatch>>::new();
        self.matcher.knn_match(&desc, &mut knn, 2, &core::no_array(), false)?;

        // 查询是帧描述子（query_idx 指向帧），训练集是模板描述子（train_idx 指向模板）。
        // 模板特征是在 2 倍上采样图上提取的，坐标需要除以上采样倍数回到模板原坐标。
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
                src_pts.push(core::Point2f::new(tp.x / TEMPLATE_UPSCALE as f32, tp.y / TEMPLATE_UPSCALE as f32));
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

        let corners = core::Vector::<core::Point2f>::from_iter([
            core::Point2f::new(0.0, 0.0),
            core::Point2f::new(self.tpl_w, 0.0),
            core::Point2f::new(self.tpl_w, self.tpl_h),
            core::Point2f::new(0.0, self.tpl_h),
        ]);
        let mut mapped = core::Vector::<core::Point2f>::new();
        core::perspective_transform(&corners, &mut mapped, &h)?;
        let quad: Quad = {
            let mut q = [[0.0f64; 2]; 4];
            for i in 0..4 {
                let p = mapped.get(i)?;
                q[i] = [p.x as f64, p.y as f64];
            }
            q
        };
        if !is_convex(&quad) || quad_area(&quad) < 16.0 {
            return Ok(None);
        }

        // 重投影误差：内点在模板上的点经 H 映射后与帧内匹配点的平均距离
        let mut err_sum = 0.0f64;
        let mut err_n = 0usize;
        for i in 0..src_pts.len() {
            if *mask.at::<u8>(i as i32)? == 0 {
                continue;
            }
            let s = src_pts.get(i)?;
            let d = dst_pts.get(i)?;
            let mut proj = core::Vector::<core::Point2f>::from_iter([s]);
            let mut out = core::Vector::<core::Point2f>::new();
            core::perspective_transform(&proj, &mut out, &h)?;
            let p = out.get(0)?;
            err_sum += (((p.x - d.x).powi(2) + (p.y - d.y).powi(2)) as f64).sqrt();
            err_n += 1;
            proj.clear();
        }
        let reproj = if err_n > 0 { err_sum / err_n as f64 } else { f64::INFINITY };
        let conf = match_confidence(inliers, good, reproj);
        Ok(Some(SiftLocated { quad, inliers, good, reproj_error_px: reproj, conf }))
    }
}
