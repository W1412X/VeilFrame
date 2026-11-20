//! 视觉几何与匹配引擎（OpenCV）。
//!
//! - `ncc`：极速档，`matchTemplate` 多尺度 NCC。
//! - `sift`：标准档，SIFT + ratio test + RANSAC 单应。
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

/// 匹配置信度：内点数、内点率、重投影误差三者综合。
///
/// 文档只给出 `f(内点数, 内点率, 重投影误差, 时序一致性)` 的形式，没有具体公式。
/// 这里的参数是初始值，需要用基准集校准（见 ADR-0007）。
pub fn match_confidence(inliers: usize, good_matches: usize, reproj_error_px: f64) -> f64 {
    if good_matches == 0 || !reproj_error_px.is_finite() {
        return 0.0;
    }
    let ratio = inliers as f64 / good_matches as f64;
    let count_factor = (inliers as f64 / 30.0).min(1.0);
    let error_factor = (1.0 - reproj_error_px / 4.0).clamp(0.0, 1.0);
    (ratio * count_factor * error_factor).clamp(0.0, 1.0)
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
}
