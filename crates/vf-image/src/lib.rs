//! 图像公共类型：RGB 帧与 OpenCV `Mat` 之间的转换。
//!
//! 管线内部统一使用 RGB24 字节布局；需要 OpenCV 处理时再转换为 `Mat`。

use opencv::{core, imgproc, prelude::*};

#[derive(Debug, Clone, PartialEq)]
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    /// RGB24，行优先，长度 = width * height * 3。
    pub data: Vec<u8>,
}

impl RgbFrame {
    pub fn new(width: usize, height: usize, fill: [u8; 3]) -> Self {
        let mut data = Vec::with_capacity(width * height * 3);
        for _ in 0..width * height {
            data.extend_from_slice(&fill);
        }
        Self { width, height, data }
    }
}

/// RGB24 字节 → 3 通道 `Mat`（CV_8UC3，像素顺序仍为 RGB）。
pub fn mat_from_rgb(width: usize, height: usize, rgb: &[u8]) -> opencv::Result<core::Mat> {
    debug_assert_eq!(rgb.len(), width * height * 3, "RGB 数据长度与尺寸不符");
    let flat = core::Mat::from_slice(rgb)?;
    let shaped = flat.reshape(3, height as i32)?;
    shaped.try_clone()
}

/// 3 通道 `Mat`（RGB）→ RGB24 字节。
pub fn rgb_from_mat(mat: &core::Mat) -> opencv::Result<Vec<u8>> {
    let mut contiguous = core::Mat::default();
    if mat.is_continuous() {
        contiguous = mat.try_clone()?;
    } else {
        mat.copy_to(&mut contiguous)?;
    }
    Ok(contiguous.data_bytes()?.to_vec())
}

/// RGB → 灰度单通道 `Mat`。
pub fn gray_from_rgb(width: usize, height: usize, rgb: &[u8]) -> opencv::Result<core::Mat> {
    let src = mat_from_rgb(width, height, rgb)?;
    let mut gray = core::Mat::default();
    imgproc::cvt_color(&src, &mut gray, imgproc::COLOR_RGB2GRAY, 0)?;
    Ok(gray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_round_trip_through_mat_keeps_bytes() {
        let frame = RgbFrame { width: 3, height: 2, data: (0..18).collect() };
        let m = mat_from_rgb(frame.width, frame.height, &frame.data).unwrap();
        assert_eq!((m.rows(), m.cols()), (2, 3));
        assert_eq!(rgb_from_mat(&m).unwrap(), frame.data);
    }

    #[test]
    fn gray_uses_bt601_luma() {
        let g = gray_from_rgb(1, 1, &[255, 0, 0]).unwrap();
        let v = *g.at_2d::<u8>(0, 0).unwrap();
        assert!((v as i32 - 76).abs() <= 1);
    }
}
