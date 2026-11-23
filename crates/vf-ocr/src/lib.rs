//! PP-OCRv5 文字检测（DB）与识别（CTC），ONNX Runtime 推理，OpenCV 做图像处理与后处理
//! （对应 `first_design.md` 2.4）。
//!
//! 检测：归一化 → DB 概率图 → 二值化 → 轮廓 → 最小外接矩形 → 按 unclip 比例外扩 → 四边形。
//! 识别：透视裁剪为固定高度的文字行 → 归一化 → CTC 贪心解码（去空白、去重复）。
//! 方向分类（cls）暂未接入，竖排文字靠裁剪后的旋转处理。

use std::path::Path;
use std::sync::Mutex;

use ort::{environment::Environment, session::Session, value::Tensor};
use opencv::{core, imgproc, prelude::*};
use vf_project::Quad;
use vf_text::{OcrEngine, OcrLine};

/// 检测输入的最长边（像素），超过则等比缩小。
pub const DET_MAX_SIDE: i32 = 960;
/// DB 二值化阈值。
pub const DB_THRESH: f64 = 0.3;
/// 文字框的平均概率阈值。
pub const DB_BOX_THRESH: f64 = 0.5;
/// 框外扩比例（unclip）。
pub const DB_UNCLIP_RATIO: f64 = 1.6;
/// 识别输入高度（PP-OCR 固定为 48）。
pub const REC_HEIGHT: i32 = 48;
/// 识别输入最大宽度。
pub const REC_MAX_WIDTH: i32 = 320;
/// 行级文字置信度阈值（配置 text_score）。
pub const TEXT_SCORE: f64 = 0.5;

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("ONNX Runtime 错误：{0}")]
    Ort(String),
    #[error("OpenCV 错误：{0}")]
    Cv(String),
    #[error("字典文件无效：{0}")]
    Dict(String),
    #[error("模型输出形状异常：{0}")]
    Shape(String),
}

fn ort_err(e: impl std::fmt::Display) -> OcrError {
    OcrError::Ort(e.to_string())
}

fn cv_err(e: impl std::fmt::Display) -> OcrError {
    OcrError::Cv(e.to_string())
}

/// 进程内唯一的 ONNX Runtime 环境。ORT 每个进程只允许创建一个环境，
/// 因此所有 OCR 会话（每个检测线程一个）共享它。环境随进程存活，这里有意不释放。
static ORT_ENV: Mutex<Option<&'static Environment>> = Mutex::new(None);

/// 返回共享环境；首次调用时加载 ONNX Runtime 动态库。之后的 `ort_lib` 参数被忽略。
fn shared_environment(ort_lib: &Path) -> Result<&'static Environment, OcrError> {
    let mut slot = ORT_ENV.lock().map_err(|_| OcrError::Ort("ONNX Runtime 环境锁已损坏".into()))?;
    if let Some(env) = *slot {
        return Ok(env);
    }
    let env = ort::init_from(ort_lib).map_err(ort_err)?.build().map_err(ort_err)?;
    let env: &'static Environment = Box::leak(Box::new(env));
    *slot = Some(env);
    Ok(env)
}

fn commit_session(env: &Environment, model: &Path, intra_threads: Option<usize>) -> Result<Session, OcrError> {
    let mut builder = Session::builder(env).map_err(ort_err)?;
    if let Some(n) = intra_threads {
        builder = builder.with_intra_threads(n).map_err(ort_err)?;
    }
    builder.commit_from_file(model).map_err(ort_err)
}

pub struct PpOcr {
    det: Session,
    rec: Session,
    /// 字符表（不含 CTC 空白符）。索引 i 对应模型输出的类别 i+1。
    dict: Vec<String>,
}

impl PpOcr {
    /// 加载两个模型并读取字典。每个会话独立，可在不同线程中各自创建。
    /// `intra_threads` 为单个会话内部的线程数，`None` 使用 ONNX Runtime 默认值。
    pub fn load(
        ort_lib: &Path,
        det_model: &Path,
        rec_model: &Path,
        dict_file: &Path,
        intra_threads: Option<usize>,
    ) -> Result<Self, OcrError> {
        let env = shared_environment(ort_lib)?;
        let det = commit_session(env, det_model, intra_threads)?;
        let rec = commit_session(env, rec_model, intra_threads)?;
        let dict = load_dict(dict_file)?;
        Ok(Self { det, rec, dict })
    }

    /// 检测文字框，返回原图坐标下的四边形。
    fn detect(&mut self, img: &core::Mat) -> Result<Vec<Quad>, OcrError> {
        let (h, w) = (img.rows(), img.cols());
        let scale = (DET_MAX_SIDE as f64 / h.max(w) as f64).min(1.0);
        let rh = ((((h as f64) * scale) / 32.0).round() as i32 * 32).max(32);
        let rw = ((((w as f64) * scale) / 32.0).round() as i32 * 32).max(32);

        let mut resized = core::Mat::default();
        imgproc::resize(img, &mut resized, core::Size::new(rw, rh), 0.0, 0.0, imgproc::INTER_LINEAR)
            .map_err(cv_err)?;
        let input = to_nchw(&resized)?;
        let tensor = Tensor::from_array(([1usize, 3, rh as usize, rw as usize], input)).map_err(ort_err)?;
        let outputs = self.det.run(ort::inputs![tensor]).map_err(ort_err)?;
        let (_, prob_data) = outputs[0].try_extract_tensor::<f32>().map_err(ort_err)?;
        let flat = core::Mat::from_slice(prob_data).map_err(cv_err)?;
        let prob = flat.reshape(1, rh).map_err(cv_err)?.try_clone().map_err(cv_err)?;

        let mut bin = core::Mat::default();
        imgproc::threshold(&prob, &mut bin, DB_THRESH, 255.0, imgproc::THRESH_BINARY).map_err(cv_err)?;
        let mut bin8 = core::Mat::default();
        bin.convert_to(&mut bin8, core::CV_8U, 1.0, 0.0).map_err(cv_err)?;

        let mut contours = core::Vector::<core::Vector<core::Point>>::new();
        imgproc::find_contours(
            &bin8,
            &mut contours,
            imgproc::RETR_LIST,
            imgproc::CHAIN_APPROX_SIMPLE,
            core::Point::default(),
        )
        .map_err(cv_err)?;

        let sx = w as f64 / rw as f64;
        let sy = h as f64 / rh as f64;
        let mut quads = Vec::new();
        for c in contours.iter() {
            let area = imgproc::contour_area(&c, false).map_err(cv_err)?;
            if area < 10.0 {
                continue;
            }
            let rect = imgproc::min_area_rect(&c).map_err(cv_err)?;
            let bbox = rect.bounding_rect().map_err(cv_err)?;
            let roi_rect = clamp_rect(bbox, rw, rh);
            let roi = core::Mat::roi(&prob, roi_rect).map_err(cv_err)?;
            let score = core::mean(&roi, &core::no_array()).map_err(cv_err)?[0];
            if score < DB_BOX_THRESH {
                continue;
            }
            // unclip：按面积与周长比例向外扩张（DB 论文的标准做法）
            let perimeter = imgproc::arc_length(&c, true).map_err(cv_err)?;
            let d = if perimeter > 0.0 { area * DB_UNCLIP_RATIO / perimeter } else { 0.0 } as f32;
            let expanded = core::RotatedRect::new(
                rect.center,
                core::Size2f::new(rect.size.width + 2.0 * d, rect.size.height + 2.0 * d),
                rect.angle,
            )
            .map_err(cv_err)?;
            let mut pts = core::Mat::default();
            imgproc::box_points(expanded, &mut pts).map_err(cv_err)?;
            let mut raw = [[0.0f64; 2]; 4];
            for (i, row) in raw.iter_mut().enumerate() {
                let p = pts.at_2d::<f32>(i as i32, 0).map_err(cv_err)?;
                let q = pts.at_2d::<f32>(i as i32, 1).map_err(cv_err)?;
                *row = [(*p as f64) * sx, (*q as f64) * sy];
            }
            quads.push(order_quad(raw));
        }
        Ok(quads)
    }

    /// 识别一个四边形区域内的文字，返回 (文字, 平均置信度)。
    fn recognize_crop(&mut self, img: &core::Mat, quad: &Quad) -> Result<Option<(String, f64)>, OcrError> {
        let w = dist(quad[0], quad[1]).max(dist(quad[3], quad[2])).round() as i32;
        let h = dist(quad[0], quad[3]).max(dist(quad[1], quad[2])).round() as i32;
        if w < 2 || h < 2 {
            return Ok(None);
        }
        let src = core::Vector::<core::Point2f>::from_iter(
            quad.iter().map(|p| core::Point2f::new(p[0] as f32, p[1] as f32)),
        );
        let dst = core::Vector::<core::Point2f>::from_iter([
            core::Point2f::new(0.0, 0.0),
            core::Point2f::new(w as f32, 0.0),
            core::Point2f::new(w as f32, h as f32),
            core::Point2f::new(0.0, h as f32),
        ]);
        let m = imgproc::get_perspective_transform(&src, &dst, core::DECOMP_LU).map_err(cv_err)?;
        let mut crop = core::Mat::default();
        imgproc::warp_perspective(
            img,
            &mut crop,
            &m,
            core::Size::new(w, h),
            imgproc::INTER_CUBIC,
            core::BORDER_REPLICATE,
            core::Scalar::default(),
        )
        .map_err(cv_err)?;

        if crop.rows() as f32 >= 1.5 * crop.cols() as f32 {
            let mut rot = core::Mat::default();
            core::rotate(&crop, &mut rot, core::ROTATE_90_COUNTERCLOCKWISE).map_err(cv_err)?;
            crop = rot;
        }
        let rw = ((REC_HEIGHT as f64 * crop.cols() as f64 / crop.rows() as f64).ceil() as i32)
            .clamp(8, REC_MAX_WIDTH);
        let mut norm = core::Mat::default();
        imgproc::resize(&crop, &mut norm, core::Size::new(rw, REC_HEIGHT), 0.0, 0.0, imgproc::INTER_LINEAR)
            .map_err(cv_err)?;

        let input = to_nchw(&norm)?;
        let tensor = Tensor::from_array(([1usize, 3, REC_HEIGHT as usize, rw as usize], input)).map_err(ort_err)?;
        let outputs = self.rec.run(ort::inputs![tensor]).map_err(ort_err)?;
        let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(ort_err)?;
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        if dims.len() != 3 {
            return Err(OcrError::Shape(format!("识别输出应为 3 维，实际 {dims:?}")));
        }
        let (steps, classes) = (dims[1], dims[2]);
        let data = data.to_vec();
        Ok(ctc_greedy(&data, steps, classes, &self.dict))
    }
}

/// CTC 贪心解码：每步取 argmax，去掉空白（类别 0）与相邻重复。
/// 类别 i（i ≥ 1）对应字典第 i-1 项；超出字典范围的类别视为空格（PP-OCR 字典约定）。
/// 返回 (文字, 所选类别的平均概率)，没有任何字符时返回 `None`。
pub fn ctc_greedy(data: &[f32], steps: usize, classes: usize, dict: &[String]) -> Option<(String, f64)> {
    let mut text = String::new();
    let mut conf_sum = 0.0f64;
    let mut conf_n = 0usize;
    let mut last = 0usize;
    for t in 0..steps {
        let row = &data[t * classes..(t + 1) * classes];
        let (idx, val) = row
            .iter()
            .enumerate()
            .fold((0usize, f32::MIN), |acc, (i, &v)| if v > acc.1 { (i, v) } else { acc });
        if idx != 0 && idx != last {
            match idx.checked_sub(1).and_then(|i| dict.get(i)) {
                Some(c) => text.push_str(c),
                None => text.push(' '),
            }
            conf_sum += val as f64;
            conf_n += 1;
        }
        last = idx;
    }
    if text.is_empty() {
        None
    } else {
        Some((text, conf_sum / conf_n as f64))
    }
}

impl OcrEngine for PpOcr {
    fn recognize(&mut self, width: usize, height: usize, rgb: &[u8]) -> Result<Vec<OcrLine>, String> {
        let img = vf_image::mat_from_rgb(width, height, rgb).map_err(|e| e.to_string())?;
        let quads = self.detect(&img).map_err(|e| e.to_string())?;
        let mut lines = Vec::new();
        for q in quads {
            if let Some((text, conf)) = self.recognize_crop(&img, &q).map_err(|e| e.to_string())? {
                if conf >= TEXT_SCORE && !text.trim().is_empty() {
                    lines.push(OcrLine { quad: q, text, conf });
                }
            }
        }
        Ok(lines)
    }
}

/// 允许把一次加载的 OCR 引擎借给多个目标顺序扫描，避免反复加载模型。
impl<'a> OcrEngine for &'a mut PpOcr {
    fn recognize(&mut self, width: usize, height: usize, rgb: &[u8]) -> Result<Vec<OcrLine>, String> {
        (**self).recognize(width, height, rgb)
    }
}

fn load_dict(path: &Path) -> Result<Vec<String>, OcrError> {
    let text = std::fs::read_to_string(path).map_err(|e| OcrError::Dict(e.to_string()))?;
    let dict: Vec<String> = text.lines().map(|l| l.trim_end_matches('\r').to_string()).filter(|l| !l.is_empty()).collect();
    if dict.is_empty() {
        return Err(OcrError::Dict("字典为空".into()));
    }
    Ok(dict)
}

/// RGB `Mat` → NCHW 浮点张量，归一化为 (x/255 - 0.5) / 0.5，即 [-1, 1]。
fn to_nchw(img: &core::Mat) -> Result<Vec<f32>, OcrError> {
    let (h, w) = (img.rows() as usize, img.cols() as usize);
    let bytes = img.data_bytes().map_err(cv_err)?;
    let plane = h * w;
    let mut out = vec![0f32; 3 * plane];
    for i in 0..plane {
        for c in 0..3 {
            out[c * plane + i] = (bytes[i * 3 + c] as f32 / 255.0 - 0.5) / 0.5;
        }
    }
    Ok(out)
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn clamp_rect(r: core::Rect, w: i32, h: i32) -> core::Rect {
    let x0 = r.x.clamp(0, w - 1);
    let y0 = r.y.clamp(0, h - 1);
    let x1 = (r.x + r.width).clamp(x0 + 1, w);
    let y1 = (r.y + r.height).clamp(y0 + 1, h);
    core::Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// 把四个点排成 左上、右上、右下、左下。
pub fn order_quad(p: [[f64; 2]; 4]) -> Quad {
    let sum = |q: &[f64; 2]| q[0] + q[1];
    let diff = |q: &[f64; 2]| q[1] - q[0];
    let tl = *p.iter().min_by(|a, b| sum(a).total_cmp(&sum(b))).unwrap();
    let br = *p.iter().max_by(|a, b| sum(a).total_cmp(&sum(b))).unwrap();
    let tr = *p.iter().min_by(|a, b| diff(a).total_cmp(&diff(b))).unwrap();
    let bl = *p.iter().max_by(|a, b| diff(a).total_cmp(&diff(b))).unwrap();
    [tl, tr, br, bl]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_quad_sorts_corners() {
        let shuffled = [[10.0, 10.0], [0.0, 0.0], [0.0, 10.0], [10.0, 0.0]];
        assert_eq!(order_quad(shuffled), [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
    }

    #[test]
    fn ctc_collapses_repeats_and_blanks() {
        // 类别：0 空白，1 -> '机'，2 -> '密'，3 -> 空格（字典外）
        let dict = vec!["机".to_string(), "密".to_string()];
        // 步骤：机 机 空白 密 密 -> "机密"（重复被合并，空白分隔的重复保留）
        let steps = [1usize, 1, 0, 2, 2];
        let classes = 4;
        let mut data = vec![0f32; steps.len() * classes];
        for (t, &c) in steps.iter().enumerate() {
            data[t * classes + c] = 0.9;
        }
        let (text, conf) = ctc_greedy(&data, steps.len(), classes, &dict).unwrap();
        assert_eq!(text, "机密");
        assert!((conf - 0.9).abs() < 1e-6);
    }

    #[test]
    fn ctc_keeps_repeated_chars_separated_by_blank() {
        let dict = vec!["a".to_string()];
        let steps = [1usize, 0, 1];
        let classes = 2;
        let mut data = vec![0f32; steps.len() * classes];
        for (t, &c) in steps.iter().enumerate() {
            data[t * classes + c] = 0.8;
        }
        assert_eq!(ctc_greedy(&data, 3, classes, &dict).unwrap().0, "aa");
    }

    #[test]
    fn ctc_outputs_space_for_index_past_dictionary() {
        let dict = vec!["x".to_string()];
        let classes = 3;
        let mut data = vec![0f32; classes];
        data[2] = 0.7;
        assert_eq!(ctc_greedy(&data, 1, classes, &dict).unwrap().0, " ");
    }

    #[test]
    fn ctc_returns_none_for_all_blank() {
        let data = vec![0.9f32, 0.1, 0.0, 0.9, 0.0, 0.1];
        assert!(ctc_greedy(&data, 2, 3, &["x".to_string()]).is_none());
    }
}
