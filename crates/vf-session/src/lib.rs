//! 会话层：把项目从打开到扫描、复核、导出的编排集中在这里。
//!
//! Tauri 命令只负责参数转换、事件推送与互斥，本 crate 不依赖 GUI，便于测试。
//! 扫描在代理分辨率上做，输出坐标会映射回源视频像素坐标。

use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use opencv::{core, imgproc, prelude::*};
use vf_cache::{Namespace, PerceptualCache};
use vf_core::Editor;
use vf_export::{ExportOptions, ExportReport};
use vf_image::gray_from_file;
use vf_media::stream::FfmpegPaths;
use vf_media::{sampled_content_hash, FfprobeBackend, MediaBackend};
use vf_ocr::PpOcr;
use vf_pipeline::{
    scale_fused_segments, to_segments, Detector, FfmpegFrameSource, MultiDetector, NccDetector, RoiDetector,
    ScanConfig, ScanOutcome, SiftDetector, TextDetector, ThresholdDetector,
};
use vf_project::{Engine, Keyframe, Mode, Project, Quad, Segment, SegmentStatus, Source, Target, TargetKind, TextMatchMode, Video};
use vf_text::{MatchMode, TextQuery};
use vf_vision::ncc::NccTemplate;
use vf_vision::sift::SiftMatcher;

/// OCR 模型与运行时路径。
#[derive(Debug, Clone)]
pub struct OcrModelPaths {
    pub ort_lib: PathBuf,
    pub det: PathBuf,
    pub rec: PathBuf,
    pub dict: PathBuf,
}

/// 会话配置。默认值读取环境变量，缺省指向本机 WSL 的安装位置。
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub ffmpeg: FfmpegPaths,
    pub ocr: OcrModelPaths,
    /// 感知缓存数据库；父目录不存在时自动创建。
    pub cache_db: PathBuf,
    /// 扫描/预览代理分辨率的最长边。
    pub proxy_max_side: u32,
}

fn home_dir() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn env_path(key: &str, default: impl FnOnce() -> PathBuf) -> PathBuf {
    env::var_os(key).map(PathBuf::from).unwrap_or_else(default)
}

impl Default for SessionConfig {
    fn default() -> Self {
        let home = home_dir();
        let models = env_path("VF_MODELS_DIR", || home.join(".veilframe/models"));
        let cache_db = env_path("VF_CACHE_DB", || {
            let base = env_path("XDG_CACHE_HOME", || home.join(".cache"));
            base.join("veilframe").join("cache.db")
        });
        Self {
            ffmpeg: FfmpegPaths {
                ffmpeg: env_path("VF_FFMPEG", || "ffmpeg".into()),
                ffprobe: env_path("VF_FFPROBE", || "ffprobe".into()),
            },
            ocr: OcrModelPaths {
                ort_lib: env_path("VF_ORT_LIB", || {
                    env_path("ORT_DYLIB_PATH", || home.join("opt/onnxruntime/lib/libonnxruntime.so"))
                }),
                det: env_path("VF_OCR_DET", || models.join("det_v5.onnx")),
                rec: env_path("VF_OCR_REC", || models.join("rec_v5.onnx")),
                dict: env_path("VF_OCR_DICT", || models.join("ppocr_keys_v5_onnx.txt")),
            },
            cache_db,
            proxy_max_side: 960,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("视频探测失败：{0}")]
    Media(String),
    #[error("项目文件错误：{0}")]
    Project(#[from] vf_project::ProjectError),
    #[error("编辑失败：{0}")]
    Edit(#[from] vf_core::EditError),
    #[error("扫描失败：{0}")]
    Scan(String),
    #[error("导出失败：{0}")]
    Export(#[from] vf_export::ExportError),
    #[error("项目没有源视频")]
    NoVideo,
    #[error("找不到目标 {0}")]
    UnknownTarget(String),
    #[error("文本目标 {0} 没有匹配串")]
    MissingPattern(String),
    #[error("图像目标 {0} 没有模板")]
    MissingTemplate(String),
    #[error("OCR 引擎初始化失败：{0}")]
    Ocr(String),
    #[error("项目还没有保存路径")]
    NoProjectPath,
    #[error("文件操作失败：{0}")]
    Io(String),
}

#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    /// 抽帧间隔（秒）。
    pub interval: f64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { interval: 0.5 }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanProgress {
    pub target_index: usize,
    pub target_total: usize,
    pub target_id: String,
    /// 已扫描到的时间（秒）。
    pub t: f64,
    pub duration: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ScanSummary {
    pub cancelled: bool,
    /// 被取消时最后处理过的时间；正常结束时为视频时长。
    pub scanned_until: f64,
    pub segments: usize,
    /// 感知缓存不可用时的说明（扫描仍会继续，只是不缓存）。
    pub cache_warning: Option<String>,
}

/// 一个已打开的项目及其运行时资源（感知缓存、OCR 会话按需加载）。
pub struct Session {
    editor: Editor,
    project_path: Option<PathBuf>,
    config: SessionConfig,
    cache: Option<PerceptualCache>,
    ocr: Option<PpOcr>,
    last_autosave_error: Option<String>,
}

impl Session {
    pub fn open_video(path: &Path, config: SessionConfig) -> Result<Self, SessionError> {
        let info = FfprobeBackend { bin: config.ffmpeg.ffprobe.clone() }
            .probe(path)
            .map_err(|e| SessionError::Media(e.to_string()))?;
        let content_hash = sampled_content_hash(path).map_err(|e| SessionError::Media(e.to_string()))?;
        let mut project = Project::new();
        project.video = Some(Video {
            path: path.to_string_lossy().into_owned(),
            content_hash,
            duration: info.duration,
            fps: info.fps,
            width: info.width,
            height: info.height,
        });
        Ok(Self::new(project, None, config))
    }

    pub fn open_project(path: &Path, config: SessionConfig) -> Result<Self, SessionError> {
        let project = Project::load(path)?;
        Ok(Self::new(project, Some(path.to_path_buf()), config))
    }

    fn new(project: Project, project_path: Option<PathBuf>, config: SessionConfig) -> Self {
        Self {
            editor: Editor::new(project),
            project_path,
            config,
            cache: None,
            ocr: None,
            last_autosave_error: None,
        }
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    pub fn project_path(&self) -> Option<&Path> {
        self.project_path.as_deref()
    }

    pub fn can_undo(&self) -> bool {
        self.editor.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.editor.can_redo()
    }

    pub fn last_autosave_error(&self) -> Option<&str> {
        self.last_autosave_error.as_deref()
    }

    /// 保存到指定路径；`None` 时使用已有关联路径。成功后更新关联路径。
    pub fn save(&mut self, path: Option<&Path>) -> Result<PathBuf, SessionError> {
        let target = path.map(Path::to_path_buf).or_else(|| self.project_path.clone()).ok_or(SessionError::NoProjectPath)?;
        self.editor.project().save(&target)?;
        self.project_path = Some(target.clone());
        self.last_autosave_error = None;
        Ok(target)
    }

    /// 有关联路径时静默保存（用于编辑后的自动保存），失败只记录不打断操作。
    fn autosave(&mut self) {
        let Some(path) = self.project_path.clone() else { return };
        match self.editor.project().save(&path) {
            Ok(()) => self.last_autosave_error = None,
            Err(e) => self.last_autosave_error = Some(e.to_string()),
        }
    }

    pub fn undo(&mut self) -> Result<(), SessionError> {
        self.editor.undo()?;
        self.autosave();
        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), SessionError> {
        self.editor.redo()?;
        self.autosave();
        Ok(())
    }

    pub fn add_target(&mut self, target: Target) -> Result<(), SessionError> {
        self.editor.add_target(target)?;
        self.autosave();
        Ok(())
    }

    pub fn update_target(&mut self, target: Target) -> Result<(), SessionError> {
        self.editor.update_target(target)?;
        self.autosave();
        Ok(())
    }

    pub fn accept(&mut self, segment_id: &str) -> Result<(), SessionError> {
        self.editor.accept(segment_id)?;
        self.autosave();
        Ok(())
    }

    pub fn reject(&mut self, segment_id: &str, negative: bool) -> Result<(), SessionError> {
        self.editor.reject(segment_id, negative)?;
        self.autosave();
        Ok(())
    }

    pub fn delete_segment(&mut self, segment_id: &str) -> Result<(), SessionError> {
        self.editor.delete_segment(segment_id)?;
        self.autosave();
        Ok(())
    }

    pub fn extend_track(&mut self, segment_id: &str) -> Result<(), SessionError> {
        self.editor.extend_track(segment_id)?;
        self.autosave();
        Ok(())
    }

    pub fn set_keyframe(&mut self, segment_id: &str, t: f64, quad: Quad) -> Result<(), SessionError> {
        self.editor.set_keyframe(segment_id, t, quad)?;
        self.autosave();
        Ok(())
    }

    pub fn add_manual_segment(&mut self, id: &str, target_id: &str, t0: f64, t1: f64, quad: Quad) -> Result<(), SessionError> {
        self.editor.add_manual_segment(id, target_id, t0, t1, quad)?;
        self.autosave();
        Ok(())
    }

    /// 扫描若干目标，把结果替换为这些目标的自动段（人工段保留）。可取消。
    pub fn scan_targets(
        &mut self,
        target_ids: &[String],
        opts: ScanOptions,
        cancel: &AtomicBool,
        mut progress: impl FnMut(ScanProgress),
    ) -> Result<ScanSummary, SessionError> {
        let video = self.project().video.clone().ok_or(SessionError::NoVideo)?;
        if video.duration <= 0.0 {
            return Err(SessionError::Scan("视频时长为 0，无法扫描".into()));
        }
        if target_ids.is_empty() {
            return Ok(ScanSummary { scanned_until: video.duration, ..Default::default() });
        }
        for id in target_ids {
            if self.project().target(id).is_none() {
                return Err(SessionError::UnknownTarget(id.clone()));
            }
        }

        let (pw, ph) = proxy_size(video.width, video.height, self.config.proxy_max_side);
        let (sx, sy) = (video.width as f64 / pw as f64, video.height as f64 / ph as f64);
        let cache_warning = self.ensure_cache();
        let interval = opts.interval.max(0.05);
        let project_dir = self.project_path.as_deref().and_then(Path::parent).map(Path::to_path_buf);

        let (built, cache_warning, stopped) = {
            let Session { editor, config, cache, ocr, .. } = self;
            let project = editor.project();
            let mut built: Vec<(String, Vec<Segment>, bool, f64)> = Vec::new();
            let total = target_ids.len();
            let mut stopped = false;

            for (idx, tid) in target_ids.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    stopped = true;
                    break;
                }
                let target = project.target(tid).expect("已校验目标存在").clone();
                if target.kind == TargetKind::Region {
                    // 固定区域不需要检测：直接生成覆盖全片的一段，框即区域（缺省整帧）。
                    progress(ScanProgress {
                        target_index: idx,
                        target_total: total,
                        target_id: tid.clone(),
                        t: 0.0,
                        duration: video.duration,
                    });
                    built.push((tid.clone(), vec![region_segment(&target, &video)], false, video.duration));
                    continue;
                }
                let (mut detector, engine, template_fingerprint) =
                    build_detector(&target, &video, project_dir.as_deref(), config, pw, ph, ocr)?;

                let params = serde_json::json!({
                    "target": target,
                    "proxy": [pw, ph],
                    "interval": interval,
                    "templates": template_fingerprint,
                })
                .to_string();
                let ns = Namespace::new(&video.content_hash, engine_version(engine), &params);

                let mut source = FfmpegFrameSource::open(
                    &config.ffmpeg.ffmpeg,
                    Path::new(&video.path),
                    1.0 / interval,
                    Some((pw as usize, ph as usize)),
                )
                .map_err(SessionError::Scan)?;

                let cfg = ScanConfig {
                    duration: video.duration,
                    interval,
                    fusion: Default::default(),
                    cache: cache.as_ref().map(|c| (c, &ns)),
                };
                let out = vf_pipeline::scan(&mut source, &mut detector, &cfg, cancel, |t| {
                    progress(ScanProgress {
                        target_index: idx,
                        target_total: total,
                        target_id: tid.clone(),
                        t,
                        duration: video.duration,
                    });
                })
                .map_err(|e| SessionError::Scan(e.to_string()))?;

                let (mut fused, cancelled, until) = match out {
                    ScanOutcome::Finished(f) => (f, false, video.duration),
                    ScanOutcome::Cancelled { scanned_until, partial } => (partial, true, scanned_until),
                };
                scale_fused_segments(&mut fused, sx, sy);
                built.push((tid.clone(), to_segments(tid, &fused, engine), cancelled, until));
                if cancelled {
                    // 已取消：保留当前目标的局部结果，未开始的目标不再处理。
                    stopped = true;
                    break;
                }
            }
            (built, cache_warning, stopped)
        };

        let mut summary = ScanSummary { cache_warning, ..Default::default() };
        for (tid, segments, cancelled, until) in built {
            summary.segments += segments.len();
            summary.cancelled |= cancelled;
            summary.scanned_until = summary.scanned_until.max(until);
            self.editor.replace_auto_segments(&tid, segments)?;
        }
        summary.cancelled |= stopped;
        self.autosave();
        Ok(summary)
    }

    /// 导出当前项目。`progress` 参数为（已处理帧数，总帧数）。
    pub fn export(
        &self,
        output: &Path,
        opts: &ExportOptions,
        cancel: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> Result<ExportReport, SessionError> {
        Ok(vf_export::export(self.project(), output, opts, cancel, progress)?)
    }

    /// 打开（或创建）感知缓存。失败时返回说明并继续无缓存扫描。
    fn ensure_cache(&mut self) -> Option<String> {
        if self.cache.is_some() {
            return None;
        }
        let path = self.config.cache_db.clone();
        let result = (|| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            PerceptualCache::open(&path).map_err(|e| e.to_string())
        })();
        match result {
            Ok(c) => {
                self.cache = Some(c);
                None
            }
            Err(e) => Some(format!("感知缓存不可用（{}）：{e}", path.display())),
        }
    }
}

/// 代理分辨率：最长边不超过 `max_side`，保持宽高比，最小 2 像素。
pub fn proxy_size(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let w = w.max(2);
    let h = h.max(2);
    let longest = w.max(h);
    if longest <= max_side {
        return (w, h);
    }
    let s = max_side as f64 / longest as f64;
    (((w as f64 * s).round() as u32).max(2), ((h as f64 * s).round() as u32).max(2))
}

fn region_segment(target: &Target, video: &Video) -> Segment {
    let (w, h) = (video.width as f64, video.height as f64);
    let [x, y, rw, rh] = target.roi.map_or([0, 0, video.width, video.height], |r| r);
    let (x, y, rw, rh) = (x as f64, y as f64, rw as f64, rh as f64);
    let quad = [[x, y], [x + rw, y], [x + rw, y + rh], [x, y + rh]];
    // 防止 ROI 超出视频范围时被校验拒绝
    let clamp = |v: f64, max: f64| v.max(0.0).min(max);
    let quad = quad.map(|[px, py]| [clamp(px, w), clamp(py, h)]);
    Segment {
        id: format!("{}-region", target.id),
        target: target.id.clone(),
        t0: 0.0,
        t1: video.duration,
        status: SegmentStatus::Pending,
        source: Source::Auto,
        engine: Engine::Auto,
        conf: 1.0,
        keyframes: vec![Keyframe { t: 0.0, quad, conf: 1.0 }],
        negatives: vec![],
    }
}

fn engine_version(engine: Engine) -> &'static str {
    match engine {
        Engine::Ocr => "ocr-ppv5-1",
        Engine::Ncc => "ncc-1",
        _ => "sift-1",
    }
}

fn text_query(target: &Target) -> Result<TextQuery, SessionError> {
    let pattern = target.pattern.as_deref().ok_or_else(|| SessionError::MissingPattern(target.id.clone()))?;
    let mode = match target.match_mode {
        TextMatchMode::Contains => MatchMode::Contains,
        TextMatchMode::Exact => MatchMode::Exact,
        TextMatchMode::Regex => MatchMode::Regex,
        TextMatchMode::Fuzzy => MatchMode::Fuzzy { max_edits: target.fuzzy_edits.max(1) as usize },
    };
    TextQuery::new(pattern, mode, true).map_err(|e| SessionError::Scan(e.to_string()))
}

fn resolve_variant(path: &str, project_dir: Option<&Path>) -> PathBuf {
    let p = Path::new(path);
    match (p.is_absolute(), project_dir) {
        (false, Some(dir)) => dir.join(p),
        _ => p.to_path_buf(),
    }
}

fn file_fingerprint(path: &Path) -> Result<String, SessionError> {
    let bytes = std::fs::read(path).map_err(|e| SessionError::Io(format!("{}：{e}", path.display())))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

/// 构造某目标的检测器（含 ROI、阈值、模板变体组合），并返回实际使用的引擎与模板指纹。
fn build_detector<'a>(
    target: &Target,
    video: &Video,
    project_dir: Option<&Path>,
    config: &SessionConfig,
    pw: u32,
    ph: u32,
    ocr: &'a mut Option<PpOcr>,
) -> Result<(Box<dyn Detector + 'a>, Engine, String), SessionError> {
    let (sx, sy) = (pw as f64 / video.width.max(1) as f64, ph as f64 / video.height.max(1) as f64);
    let (inner, engine, fingerprint): (Box<dyn Detector + 'a>, Engine, String) = match target.kind {
        TargetKind::Text => {
            let query = text_query(target)?;
            if ocr.is_none() {
                *ocr = Some(
                    PpOcr::load(&config.ocr.ort_lib, &config.ocr.det, &config.ocr.rec, &config.ocr.dict)
                        .map_err(|e| SessionError::Ocr(e.to_string()))?,
                );
            }
            let engine = ocr.as_mut().expect("OCR 已加载");
            (Box::new(TextDetector { ocr: engine, query }), Engine::Ocr, String::new())
        }
        TargetKind::Image => {
            if target.variants.is_empty() {
                return Err(SessionError::MissingTemplate(target.id.clone()));
            }
            let use_ncc = target.engine == Engine::Ncc || target.mode == Mode::Fast;
            let engine = if use_ncc { Engine::Ncc } else { Engine::SiftRansac };
            let mut detectors: Vec<Box<dyn Detector + 'static>> = Vec::new();
            let mut hashes = Vec::new();
            for variant in &target.variants {
                let path = resolve_variant(variant, project_dir);
                hashes.push(file_fingerprint(&path)?);
                let mut gray = gray_from_file(&path).map_err(|e| SessionError::Scan(e.to_string()))?;
                if use_ncc {
                    let factor = pw as f64 / video.width.max(1) as f64;
                    if (factor - 1.0).abs() > 1e-6 {
                        let mut scaled = core::Mat::default();
                        imgproc::resize(
                            &gray,
                            &mut scaled,
                            core::Size::new(0, 0),
                            factor,
                            factor,
                            imgproc::INTER_AREA,
                        )
                        .map_err(|e| SessionError::Scan(e.to_string()))?;
                        gray = scaled;
                    }
                    if gray.rows() < 2 || gray.cols() < 2 {
                        return Err(SessionError::Scan(format!("模板 {variant} 尺寸过小")));
                    }
                    detectors.push(Box::new(NccDetector {
                        template: NccTemplate::new(gray, vec![0.9, 1.0, 1.1], 0.0, 8),
                    }));
                } else {
                    let matcher = SiftMatcher::new(&gray).map_err(|e| SessionError::Scan(e.to_string()))?;
                    detectors.push(Box::new(SiftDetector { matcher }));
                }
            }
            (Box::new(MultiDetector { detectors }), engine, hashes.join(","))
        }
        TargetKind::Region => {
            // 固定区域不参与检测；由人工段表达。
            (Box::new(MultiDetector { detectors: Vec::new() }), Engine::Auto, String::new())
        }
    };

    let mut det = inner;
    if let Some(roi) = target.roi {
        let r = (
            (roi[0] as f64 * sx).round().max(0.0) as u32,
            (roi[1] as f64 * sy).round().max(0.0) as u32,
            (roi[2] as f64 * sx).round().max(1.0) as u32,
            (roi[3] as f64 * sy).round().max(1.0) as u32,
        );
        det = Box::new(RoiDetector { inner: det, roi: r });
    }
    det = Box::new(ThresholdDetector { inner: det, min_conf: target.threshold });
    Ok((det, engine, fingerprint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_project::{MaskKind, Style};

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("vf-session-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn proxy_size_keeps_aspect_and_caps_long_side() {
        assert_eq!(proxy_size(1920, 1080, 960), (960, 540));
        assert_eq!(proxy_size(1080, 1920, 960), (540, 960));
        assert_eq!(proxy_size(640, 360, 960), (640, 360));
        assert_eq!(proxy_size(0, 0, 960), (2, 2));
    }

    #[test]
    fn resolve_variant_joins_relative_paths_to_project_dir() {
        let dir = Path::new("/projects/demo");
        assert_eq!(resolve_variant("logo.png", Some(dir)), PathBuf::from("/projects/demo/logo.png"));
        assert_eq!(resolve_variant("/abs/logo.png", Some(dir)), PathBuf::from("/abs/logo.png"));
        assert_eq!(resolve_variant("logo.png", None), PathBuf::from("logo.png"));
    }

    fn project_with_text_target(dir: &Path) -> (Project, PathBuf) {
        let mut p = Project::new();
        p.video = Some(Video {
            path: dir.join("in.mp4").to_string_lossy().into_owned(),
            content_hash: "blake3-sampled:test".into(),
            duration: 10.0,
            fps: 10.0,
            width: 320,
            height: 240,
        });
        p.targets.push(Target {
            id: "t1".into(),
            name: "机密".into(),
            kind: TargetKind::Text,
            color: "#d1495b".into(),
            variants: vec![],
            pattern: Some("机密".into()),
            match_mode: TextMatchMode::Contains,
            fuzzy_edits: 1,
            engine: Engine::Ocr,
            mode: Mode::Standard,
            threshold: 0.6,
            roi: None,
            style: Style { kind: MaskKind::Mosaic, strength: 0.6, padding: 4, feather: true, motion_pad: false },
            enabled: true,
        });
        let path = dir.join("demo.maskproj");
        (p, path)
    }

    #[test]
    fn save_load_and_edit_round_trip_with_autosave() {
        let dir = tmp_dir("save");
        let (p, path) = project_with_text_target(&dir);
        p.save(&path).unwrap();

        let mut s = Session::open_project(&path, SessionConfig::default()).unwrap();
        s.add_manual_segment("m1", "t1", 1.0, 2.0, [[0.0; 2]; 4]).unwrap();
        // 自动保存：磁盘上的项目应包含新段
        assert!(Project::load(&path).unwrap().segment("m1").is_some());
        assert!(s.can_undo());

        s.undo().unwrap();
        assert!(Project::load(&path).unwrap().segment("m1").is_none());
        s.redo().unwrap();
        assert!(Project::load(&path).unwrap().segment("m1").is_some());

        let saved = s.save(None).unwrap();
        assert_eq!(saved, path);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_without_path_is_an_error() {
        let dir = tmp_dir("nosave");
        let (p, _) = project_with_text_target(&dir);
        let mut s = Session::new(p, None, SessionConfig::default());
        assert!(matches!(s.save(None), Err(SessionError::NoProjectPath)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn accepting_and_rejecting_update_snapshot_flags() {
        let dir = tmp_dir("edit");
        let (mut p, path) = project_with_text_target(&dir);
        p.segments.push(Segment {
            id: "s1".into(),
            target: "t1".into(),
            t0: 0.0,
            t1: 1.0,
            status: SegmentStatus::Pending,
            source: Source::Auto,
            engine: Engine::Ocr,
            conf: 0.9,
            keyframes: vec![Keyframe { t: 0.0, quad: [[0.0; 2]; 4], conf: 0.9 }],
            negatives: vec![],
        });
        p.save(&path).unwrap();
        let mut s = Session::open_project(&path, SessionConfig::default()).unwrap();
        s.accept("s1").unwrap();
        assert_eq!(s.project().segment("s1").unwrap().status, SegmentStatus::Confirmed);
        s.reject("s1", true).unwrap();
        let proj = s.project();
        assert_eq!(proj.segment("s1").unwrap().status, SegmentStatus::Rejected);
        assert!((proj.target("t1").unwrap().threshold - 0.61).abs() < 1e-9);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_pattern_is_reported_before_scanning() {
        let dir = tmp_dir("pattern");
        let (mut p, _) = project_with_text_target(&dir);
        p.targets[0].pattern = None;
        let config = SessionConfig { cache_db: dir.join("cache.db"), ..Default::default() };
        let mut s = Session::new(p, None, config);
        let cancel = AtomicBool::new(false);
        let err = s.scan_targets(&["t1".into()], ScanOptions::default(), &cancel, |_| {}).unwrap_err();
        assert!(matches!(err, SessionError::MissingPattern(_)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn region_target_scans_to_one_full_length_segment() {
        let dir = tmp_dir("region");
        let (mut p, _) = project_with_text_target(&dir);
        let base = p.targets[0].clone();
        p.targets[0] = Target {
            kind: TargetKind::Region,
            pattern: None,
            engine: Engine::Auto,
            roi: Some([10, 20, 100, 50]),
            ..base
        };
        let config = SessionConfig { cache_db: dir.join("cache.db"), ..Default::default() };
        let mut s = Session::new(p, None, config);
        let cancel = AtomicBool::new(false);
        let summary = s.scan_targets(&["t1".into()], ScanOptions::default(), &cancel, |_| {}).unwrap();
        assert_eq!(summary.segments, 1);
        let seg = s.project().segments.first().unwrap();
        assert_eq!((seg.t0, seg.t1), (0.0, 10.0));
        assert_eq!(seg.keyframes[0].quad, [[10.0, 20.0], [110.0, 20.0], [110.0, 70.0], [10.0, 70.0]]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pre_cancelled_scan_skips_all_targets() {
        let dir = tmp_dir("cancel");
        let (mut p, _) = project_with_text_target(&dir);
        p.targets[0] = Target { kind: TargetKind::Region, pattern: None, engine: Engine::Auto, ..p.targets[0].clone() };
        let config = SessionConfig { cache_db: dir.join("cache.db"), ..Default::default() };
        let mut s = Session::new(p, None, config);
        let cancel = AtomicBool::new(true);
        let summary = s.scan_targets(&["t1".into()], ScanOptions::default(), &cancel, |_| {}).unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.segments, 0);
        assert!(s.project().segments.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn text_query_maps_modes() {
        let (p, _) = project_with_text_target(Path::new("/tmp"));
        let q = text_query(&p.targets[0]).unwrap();
        assert!(q.find("这里是机密文件").is_some());
        assert!(q.find("这里没有").is_none());
    }
}
