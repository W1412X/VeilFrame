//! 图像公共类型：RGB 帧与 OpenCV `Mat` 之间的转换。
//!
//! 管线内部统一使用 RGB24 字节布局；需要 OpenCV 处理时再转换为 `Mat`。

use std::path::Path;

use opencv::{core, imgcodecs, imgproc, prelude::*};

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("OpenCV 错误：{0}")]
    Cv(String),
    #[error("无法读取图像：{0}")]
    Unreadable(String),
}

impl From<opencv::Error> for ImageError {
    fn from(e: opencv::Error) -> Self {
        ImageError::Cv(e.to_string())
    }
}

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
    // OpenCV 4.11 起 cvtColor 多一个 AlgorithmHint 参数，见 build/opencv_version.rs。
    #[cfg(opencv_ge411)]
    imgproc::cvt_color(
        &src,
        &mut gray,
        imgproc::COLOR_RGB2GRAY,
        0,
        core::AlgorithmHint::ALGO_HINT_DEFAULT,
    )?;
    #[cfg(not(opencv_ge411))]
    imgproc::cvt_color(&src, &mut gray, imgproc::COLOR_RGB2GRAY, 0)?;
    Ok(gray)
}

/// 从磁盘读取图像并转为单通道灰度图。文件不存在或格式不支持时报错（`imread` 此时返回空 Mat）。
pub fn gray_from_file(path: &Path) -> Result<core::Mat, ImageError> {
    let text = path.display().to_string();
    let mat = imgcodecs::imread(&text, imgcodecs::IMREAD_GRAYSCALE)?;
    if mat.empty() {
        return Err(ImageError::Unreadable(text));
    }
    Ok(mat)
}

/// 裁剪 RGB 帧的矩形区域。超出边界的部分被截断；区域为空时返回 `None`。
pub fn crop_rgb(frame: &RgbFrame, x: u32, y: u32, w: u32, h: u32) -> Option<RgbFrame> {
    let x0 = (x as usize).min(frame.width);
    let y0 = (y as usize).min(frame.height);
    let x1 = ((x as usize).saturating_add(w as usize)).min(frame.width);
    let y1 = ((y as usize).saturating_add(h as usize)).min(frame.height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut data = Vec::with_capacity(cw * ch * 3);
    for row in y0..y1 {
        let start = (row * frame.width + x0) * 3;
        data.extend_from_slice(&frame.data[start..start + cw * 3]);
    }
    Some(RgbFrame { width: cw, height: ch, data })
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

    #[test]
    fn crop_copies_requested_rows() {
        // 4x3 图，每个像素的 R 通道记录列号，G 通道记录行号
        let mut data = Vec::new();
        for y in 0..3u8 {
            for x in 0..4u8 {
                data.extend_from_slice(&[x, y, 0]);
            }
        }
        let frame = RgbFrame { width: 4, height: 3, data };
        let c = crop_rgb(&frame, 1, 1, 2, 2).unwrap();
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(&c.data[0..3], &[1, 1, 0]);
        assert_eq!(&c.data[3..6], &[2, 1, 0]);
        assert_eq!(&c.data[6..9], &[1, 2, 0]);
    }

    #[test]
    fn crop_clamps_to_bounds_and_rejects_empty() {
        let frame = RgbFrame::new(4, 3, [0, 0, 0]);
        let c = crop_rgb(&frame, 3, 2, 100, 100).unwrap();
        assert_eq!((c.width, c.height), (1, 1));
        assert!(crop_rgb(&frame, 4, 0, 2, 2).is_none());
        assert!(crop_rgb(&frame, 0, 0, 0, 2).is_none());
    }

    #[test]
    fn missing_image_file_is_an_error() {
        assert!(matches!(
            gray_from_file(std::path::Path::new("/definitely/not/here.png")),
            Err(ImageError::Unreadable(_))
        ));
    }
}
