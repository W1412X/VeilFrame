//! `.maskproj` 项目文件：数据模型、校验与原子保存。
//!
//! 对应 `first_design.md` 2.8 的数据模型。原视频只读，项目文件只记录引用与决策。

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

/// 当前项目文件格式版本。
pub const FORMAT_VERSION: u32 = 1;

/// 归一化坐标下的四边形，顺序为左上、右上、右下、左下（像素坐标）。
pub type Quad = [[f64; 2]; 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Text,
    Image,
    Region,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Fast,
    Standard,
    Precise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Auto,
    Ncc,
    SiftRansac,
    Akaze,
    SuperPointLightGlue,
    ShapeEdge,
    Ocr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskKind {
    Mosaic,
    Blur,
    Solid,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentStatus {
    Confirmed,
    Pending,
    Low,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Auto,
    Manual,
}

/// 打码样式。`strength` 取值范围 0.0..=1.0，`padding` 为外扩像素。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub kind: MaskKind,
    pub strength: f64,
    pub padding: u32,
    pub feather: bool,
    pub motion_pad: bool,
}

/// 检测目标（规则）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub kind: TargetKind,
    pub color: String,
    /// 图像目标的模板变体，相对项目目录的路径。
    #[serde(default)]
    pub variants: Vec<String>,
    pub engine: Engine,
    pub mode: Mode,
    pub threshold: f64,
    /// 搜索区域 `[x, y, w, h]`，像素坐标；缺省为整帧。
    #[serde(default)]
    pub roi: Option<[u32; 4]>,
    pub style: Style,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// 关键帧：某一时刻的四边形与置信度。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub t: f64,
    pub quad: Quad,
    pub conf: f64,
}

/// 检测段：同一目标在一个时间区间内的连续出现。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub target: String,
    pub t0: f64,
    pub t1: f64,
    pub status: SegmentStatus,
    pub source: Source,
    pub engine: Engine,
    /// 段级置信度（由关键帧聚合，手动段为 1.0）。
    pub conf: f64,
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
    /// 人工否决时记录的负样本，用于收紧局部阈值。
    #[serde(default)]
    pub negatives: Vec<Keyframe>,
}

/// 源视频引用。`content_hash` 形如 `blake3-sampled:<hex>`，见 ADR-0006。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Video {
    pub path: String,
    pub content_hash: String,
    pub duration: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
}

/// 导出设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Render {
    pub codec: String,
    /// 智能渲染（仅重编码含打码的 GOP）。第一期未实现时保持 false，见 ADR-0005。
    pub smart_render: bool,
}

impl Default for Render {
    fn default() -> Self {
        Self { codec: "h264".to_string(), smart_render: false }
    }
}

/// 整个项目文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub format_version: u32,
    pub video: Option<Video>,
    #[serde(default)]
    pub targets: Vec<Target>,
    #[serde(default)]
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub render: Render,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ProjectError {
    #[error("重复的 id：{0}")]
    DuplicateId(String),
    #[error("检测段 {segment} 引用了不存在的目标 {target}")]
    UnknownTarget { segment: String, target: String },
    #[error("检测段 {0} 的时间区间无效（t0 必须小于 t1，且均在视频时长内）")]
    BadRange(String),
    #[error("检测段 {segment} 的关键帧时间 {t} 不在区间内")]
    KeyframeOutOfRange { segment: String, t: f64 },
    #[error("目标 {0} 的阈值必须在 0.0..=1.0 之间")]
    BadThreshold(String),
    #[error("目标 {0} 的打码强度必须在 0.0..=1.0 之间")]
    BadStrength(String),
    #[error("不支持的项目格式版本：{0}")]
    UnsupportedVersion(u32),
    #[error("读写项目文件失败：{0}")]
    Io(String),
    #[error("项目文件 JSON 无效：{0}")]
    Json(String),
}

impl Project {
    pub fn new() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            video: None,
            targets: Vec::new(),
            segments: Vec::new(),
            render: Render::default(),
        }
    }

    pub fn target(&self, id: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.id == id)
    }

    pub fn segment(&self, id: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.id == id)
    }

    /// 校验不变量。所有修改在提交前都应通过校验。
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.format_version != FORMAT_VERSION {
            return Err(ProjectError::UnsupportedVersion(self.format_version));
        }
        let duration = self.video.as_ref().map(|v| v.duration);

        let mut target_ids = std::collections::HashSet::new();
        for t in &self.targets {
            if !target_ids.insert(t.id.as_str()) {
                return Err(ProjectError::DuplicateId(t.id.clone()));
            }
            if !(0.0..=1.0).contains(&t.threshold) {
                return Err(ProjectError::BadThreshold(t.id.clone()));
            }
            if !(0.0..=1.0).contains(&t.style.strength) {
                return Err(ProjectError::BadStrength(t.id.clone()));
            }
        }

        let mut segment_ids = std::collections::HashSet::new();
        for s in &self.segments {
            if !segment_ids.insert(s.id.as_str()) {
                return Err(ProjectError::DuplicateId(s.id.clone()));
            }
            if !target_ids.contains(s.target.as_str()) {
                return Err(ProjectError::UnknownTarget {
                    segment: s.id.clone(),
                    target: s.target.clone(),
                });
            }
            let in_video = duration.map_or(true, |d| s.t1 <= d + 1e-9);
            if !(s.t0 >= 0.0 && s.t0 < s.t1 && in_video) {
                return Err(ProjectError::BadRange(s.id.clone()));
            }
            for k in &s.keyframes {
                if k.t < s.t0 - 1e-9 || k.t > s.t1 + 1e-9 {
                    return Err(ProjectError::KeyframeOutOfRange { segment: s.id.clone(), t: k.t });
                }
            }
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, ProjectError> {
        serde_json::to_string_pretty(self).map_err(|e| ProjectError::Json(e.to_string()))
    }

    pub fn from_json(text: &str) -> Result<Self, ProjectError> {
        let p: Project = serde_json::from_str(text).map_err(|e| ProjectError::Json(e.to_string()))?;
        p.validate()?;
        Ok(p)
    }

    /// 原子保存：先写临时文件并落盘，再替换目标文件，避免写一半导致项目损坏。
    pub fn save(&self, path: &Path) -> Result<(), ProjectError> {
        self.validate()?;
        let json = self.to_json()?;
        let tmp = path.with_extension("maskproj.tmp");
        {
            let mut f = fs::File::create(&tmp).map_err(|e| ProjectError::Io(e.to_string()))?;
            f.write_all(json.as_bytes()).map_err(|e| ProjectError::Io(e.to_string()))?;
            f.sync_all().map_err(|e| ProjectError::Io(e.to_string()))?;
        }
        fs::rename(&tmp, path).map_err(|e| ProjectError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, ProjectError> {
        let text = fs::read_to_string(path).map_err(|e| ProjectError::Io(e.to_string()))?;
        Self::from_json(&text)
    }
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> Style {
        Style { kind: MaskKind::Mosaic, strength: 0.7, padding: 8, feather: true, motion_pad: true }
    }

    fn target(id: &str) -> Target {
        Target {
            id: id.to_string(),
            name: "机密".to_string(),
            kind: TargetKind::Text,
            color: "#d1495b".to_string(),
            variants: vec![],
            engine: Engine::Ocr,
            mode: Mode::Standard,
            threshold: 0.6,
            roi: None,
            style: style(),
            enabled: true,
        }
    }

    fn segment(id: &str, target: &str, t0: f64, t1: f64) -> Segment {
        Segment {
            id: id.to_string(),
            target: target.to_string(),
            t0,
            t1,
            status: SegmentStatus::Pending,
            source: Source::Auto,
            engine: Engine::Ocr,
            conf: 0.8,
            keyframes: vec![],
            negatives: vec![],
        }
    }

    fn project_with_video() -> Project {
        let mut p = Project::new();
        p.video = Some(Video {
            path: "in.mp4".into(),
            content_hash: "blake3-sampled:abc".into(),
            duration: 225.0,
            fps: 30.0,
            width: 1920,
            height: 1080,
        });
        p.targets.push(target("t1"));
        p
    }

    #[test]
    fn valid_project_passes() {
        let mut p = project_with_video();
        p.segments.push(segment("s1", "t1", 10.0, 12.0));
        assert!(p.validate().is_ok());
    }

    #[test]
    fn rejects_unknown_target() {
        let mut p = project_with_video();
        p.segments.push(segment("s1", "nope", 10.0, 12.0));
        assert!(matches!(p.validate(), Err(ProjectError::UnknownTarget { .. })));
    }

    #[test]
    fn rejects_bad_range_and_out_of_video() {
        let mut p = project_with_video();
        p.segments.push(segment("s1", "t1", 12.0, 10.0));
        assert!(matches!(p.validate(), Err(ProjectError::BadRange(_))));

        let mut q = project_with_video();
        q.segments.push(segment("s1", "t1", 10.0, 300.0));
        assert!(matches!(q.validate(), Err(ProjectError::BadRange(_))));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let mut p = project_with_video();
        p.targets.push(target("t1"));
        assert!(matches!(p.validate(), Err(ProjectError::DuplicateId(_))));
    }

    #[test]
    fn rejects_keyframe_outside_segment() {
        let mut p = project_with_video();
        let mut s = segment("s1", "t1", 10.0, 12.0);
        s.keyframes.push(Keyframe { t: 20.0, quad: [[0.0; 2]; 4], conf: 0.9 });
        p.segments.push(s);
        assert!(matches!(p.validate(), Err(ProjectError::KeyframeOutOfRange { .. })));
    }

    #[test]
    fn rejects_bad_threshold() {
        let mut p = project_with_video();
        p.targets[0].threshold = 1.5;
        assert!(matches!(p.validate(), Err(ProjectError::BadThreshold(_))));
    }

    #[test]
    fn json_round_trip() {
        let mut p = project_with_video();
        p.segments.push(segment("s1", "t1", 10.0, 12.0));
        let json = p.to_json().unwrap();
        let back = Project::from_json(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn save_and_load_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("vf-project-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.maskproj");
        let p = project_with_video();
        p.save(&path).unwrap();
        assert!(!dir.join("demo.maskproj.tmp").exists());
        assert_eq!(Project::load(&path).unwrap(), p);
        fs::remove_dir_all(&dir).unwrap();
    }
}
