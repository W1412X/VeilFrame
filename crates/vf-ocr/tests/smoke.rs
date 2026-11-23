//! 端到端冒烟测试：用真实的 PP-OCRv5 模型识别一张图片。
//! 默认忽略，需要设置以下环境变量后运行：
//!   VF_OCR_IMAGE  待识别图片路径（PNG/JPG）
//!   VF_OCR_DET    检测模型 ONNX
//!   VF_OCR_REC    识别模型 ONNX
//!   VF_OCR_DICT   字符表
//!   ORT_DYLIB_PATH  ONNX Runtime 动态库
//! 运行：cargo test -p vf-ocr --test smoke -- --ignored --nocapture

use std::path::PathBuf;

use opencv::{core, imgcodecs, imgproc, prelude::*};
use vf_ocr::PpOcr;
use vf_text::OcrEngine;

fn env_path(key: &str) -> PathBuf {
    PathBuf::from(std::env::var(key).unwrap_or_else(|_| panic!("未设置环境变量 {key}")))
}

#[test]
#[ignore = "需要模型、图片与 ONNX Runtime 库，见文件头部说明"]
fn recognizes_text_in_real_image() {
    let image = env_path("VF_OCR_IMAGE");
    let bgr = imgcodecs::imread(image.to_str().unwrap(), imgcodecs::IMREAD_COLOR).expect("读取图片失败");
    assert!(!bgr.empty(), "图片为空：{}", image.display());

    let mut rgb = core::Mat::default();
    imgproc::cvt_color(&bgr, &mut rgb, imgproc::COLOR_BGR2RGB, 0).unwrap();
    let (w, h) = (rgb.cols() as usize, rgb.rows() as usize);
    let data = rgb.data_bytes().unwrap().to_vec();

    let ort_lib = env_path("ORT_DYLIB_PATH");
    let mut ocr = PpOcr::load(&ort_lib, &env_path("VF_OCR_DET"), &env_path("VF_OCR_REC"), &env_path("VF_OCR_DICT"), None)
        .expect("加载 OCR 模型失败");

    let lines = ocr.recognize(w, h, &data).expect("识别失败");
    println!("识别到 {} 行文字（图片 {w}x{h}）", lines.len());
    for l in &lines {
        println!("{:.3}  {}  quad={:?}", l.conf, l.text, l.quad[0]);
    }
    assert!(!lines.is_empty(), "没有识别到任何文字");
}
