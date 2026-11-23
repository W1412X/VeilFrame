//! 导出：解码源视频 → 逐帧按项目的检测段打码 → 编码输出（对应 `first_design.md` 1.6）。
//!
//! 音频默认 `copy`，直接从源视频拷贝。输出先写临时文件，完成后原子改名，
//! 取消或失败时不会留下半成品文件。智能渲染（GOP 拼接）尚未实现，见 ADR-0005。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use vf_image::{mat_from_rgb, rgb_from_mat};
use vf_media::stream::FfmpegPaths;
use vf_media::{media_command, FfprobeBackend, MediaBackend};
use vf_project::{Project, Quad, Segment, SegmentStatus};
use vf_render::{render_region, RenderError};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("视频探测失败：{0}")]
    Probe(String),
    #[error("ffmpeg 失败：{0}")]
    Ffmpeg(String),
    #[error("打码失败：{0}")]
    Render(#[from] RenderError),
    #[error("OpenCV 错误：{0}")]
    Cv(String),
    #[error("文件操作失败：{0}")]
    Io(String),
    #[error("项目没有源视频")]
    NoVideo,
    #[error("导出已取消")]
    Cancelled,
}

impl From<opencv::Error> for ExportError {
    fn from(e: opencv::Error) -> Self {
        ExportError::Cv(e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub ffmpeg: FfmpegPaths,
    /// 视频编码器（如 `libx264`）。
    pub codec: String,
    /// 质量参数（CRF，越小质量越高）。
    pub crf: u8,
    /// 是否连待确认段一起打码。`false` 时只导出已确认（含人工）的段。
    pub include_pending: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { ffmpeg: FfmpegPaths::default(), codec: "libx264".into(), crf: 18, include_pending: true }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportReport {
    pub frames: u64,
    pub masked_regions: u64,
    pub output: PathBuf,
}

/// 线性插值得到 `t` 时刻的四边形，并给出该段在此刻的运动速度（像素/帧）。
fn quad_at(seg: &Segment, t: f64, fps: f64) -> (Quad, f64) {
    let ks = &seg.keyframes;
    if ks.is_empty() {
        return ([[0.0; 2]; 4], 0.0);
    }
    if t <= ks[0].t || ks.len() == 1 {
        return (ks[0].quad, 0.0);
    }
    if t >= ks[ks.len() - 1].t {
        return (ks[ks.len() - 1].quad, 0.0);
    }
    let i = ks.iter().position(|k| k.t > t).unwrap_or(ks.len() - 1).max(1);
    let (a, b) = (&ks[i - 1], &ks[i]);
    let f = if b.t > a.t { (t - a.t) / (b.t - a.t) } else { 0.0 };
    let mut q = [[0.0f64; 2]; 4];
    for c in 0..4 {
        for d in 0..2 {
            q[c][d] = a.quad[c][d] + (b.quad[c][d] - a.quad[c][d]) * f;
        }
    }
    let center = |qq: &Quad| [(qq[0][0] + qq[2][0]) / 2.0, (qq[0][1] + qq[2][1]) / 2.0];
    let (ca, cb) = (center(&a.quad), center(&b.quad));
    let dist = ((cb[0] - ca[0]).powi(2) + (cb[1] - ca[1]).powi(2)).sqrt();
    let dt_frames = (b.t - a.t) * fps;
    let speed = if dt_frames > 0.0 { dist / dt_frames } else { 0.0 };
    (q, speed)
}

/// 导出一个项目。`progress` 每处理一帧调用一次，参数为（已处理帧数，总帧数）。
/// `cancel` 置位后尽快停止，临时文件会被删除并返回 [`ExportError::Cancelled`]。
pub fn export(
    project: &Project,
    output: &Path,
    opts: &ExportOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<ExportReport, ExportError> {
    let video = project.video.as_ref().ok_or(ExportError::NoVideo)?;
    let input = PathBuf::from(&video.path);
    let info = FfprobeBackend { bin: opts.ffmpeg.ffprobe.clone() }
        .probe(&input)
        .map_err(|e| ExportError::Probe(e.to_string()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let fps = info.fps;
    let total = (info.duration * fps).round() as u64;

    let tmp = output.with_extension("exporting.mp4");
    let mut decoder = media_command(&opts.ffmpeg.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-an", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ExportError::Ffmpeg(e.to_string()))?;
    let mut encoder = media_command(&opts.ffmpeg.ffmpeg)
        .args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24"])
        .args(["-s", &format!("{w}x{h}"), "-r", &format!("{fps:.6}"), "-i", "-"])
        .arg("-i")
        .arg(&input)
        .args(["-map", "0:v:0", "-map", "1:a?", "-c:v", &opts.codec, "-crf", &opts.crf.to_string()])
        .args(["-pix_fmt", "yuv420p", "-c:a", "copy", "-shortest", "-f", "mp4"])
        .arg(&tmp)
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ExportError::Ffmpeg(e.to_string()))?;

    let result = run_frames(project, &mut decoder, &mut encoder, w, h, fps, total, opts.include_pending, cancel, &mut progress);
    // 无论成功与否都要回收子进程
    let _ = decoder.kill();
    let _ = decoder.wait();
    let enc_status = encoder.wait().map_err(|e| ExportError::Ffmpeg(e.to_string()))?;

    match result {
        Ok((frames, masked)) if enc_status.success() => {
            std::fs::rename(&tmp, output).map_err(|e| ExportError::Io(e.to_string()))?;
            Ok(ExportReport { frames, masked_regions: masked, output: output.to_path_buf() })
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&tmp);
            Err(ExportError::Ffmpeg(format!("编码器退出码 {:?}", enc_status.code())))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_frames(
    project: &Project,
    decoder: &mut std::process::Child,
    encoder: &mut std::process::Child,
    w: usize,
    h: usize,
    fps: f64,
    total: u64,
    include_pending: bool,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(u64, u64), ExportError> {
    let mut out = encoder.stdin.take().ok_or_else(|| ExportError::Ffmpeg("无法写入编码器".into()))?;
    let mut src = decoder.stdout.take().ok_or_else(|| ExportError::Ffmpeg("无法读取解码器".into()))?;
    let frame_len = w * h * 3;
    let mut buf = vec![0u8; frame_len];
    let mut idx: u64 = 0;
    let mut masked: u64 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        // 读满一帧，不足一帧视为结束
        let mut filled = 0usize;
        while filled < frame_len {
            let n = src.read(&mut buf[filled..]).map_err(|e| ExportError::Ffmpeg(e.to_string()))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled < frame_len {
            break;
        }

        let t = idx as f64 / fps;
        let mut mat = mat_from_rgb(w, h, &buf)?;
        for seg in &project.segments {
            if !should_mask(seg, include_pending) || t < seg.t0 || t > seg.t1 {
                continue;
            }
            let Some(target) = project.target(&seg.target) else { continue };
            if !target.enabled {
                continue;
            }
            let (quad, speed) = quad_at(seg, t, fps);
            render_region(&mut mat, &quad, &target.style, speed)?;
            masked += 1;
        }
        let bytes = rgb_from_mat(&mat)?;
        out.write_all(&bytes).map_err(|e| ExportError::Ffmpeg(e.to_string()))?;

        idx += 1;
        progress(idx, total);
    }
    drop(out);
    Ok((idx, masked))
}

/// 该段在当前导出策略下是否打码。
fn should_mask(seg: &Segment, include_pending: bool) -> bool {
    seg.status != SegmentStatus::Rejected && (include_pending || seg.status == SegmentStatus::Confirmed)
}

/// 导出前的统计：有多少段会被打码（用于界面提示）。
pub fn masked_segment_count(project: &Project, include_pending: bool) -> usize {
    project
        .segments
        .iter()
        .filter(|s| should_mask(s, include_pending))
        .filter(|s| project.target(&s.target).map_or(false, |t| t.enabled))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_project::{Keyframe, Segment, SegmentStatus, Source, Engine};

    fn seg_with(keys: Vec<(f64, Quad)>) -> Segment {
        Segment {
            id: "s".into(),
            target: "t".into(),
            t0: 0.0,
            t1: 10.0,
            status: SegmentStatus::Pending,
            source: Source::Auto,
            engine: Engine::Ocr,
            conf: 0.9,
            keyframes: keys.into_iter().map(|(t, quad)| Keyframe { t, quad, conf: 0.9 }).collect(),
            negatives: vec![],
        }
    }

    #[test]
    fn quad_interpolates_between_keyframes() {
        let a: Quad = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let b: Quad = [[20.0, 0.0], [30.0, 0.0], [30.0, 10.0], [20.0, 10.0]];
        let s = seg_with(vec![(0.0, a), (2.0, b)]);
        let (q, speed) = quad_at(&s, 1.0, 30.0);
        assert!((q[0][0] - 10.0).abs() < 1e-9);
        // 中心从 (5,5) 移到 (25,5)，共 20 px，2 秒 × 30 帧 = 60 帧，速度 1/3 px/帧
        assert!((speed - 20.0 / 60.0).abs() < 1e-9);
    }

    #[test]
    fn quad_clamps_outside_keyframe_range() {
        let a: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let s = seg_with(vec![(1.0, a)]);
        assert_eq!(quad_at(&s, 0.0, 30.0).0, a);
        assert_eq!(quad_at(&s, 9.0, 30.0).0, a);
    }

    #[test]
    fn masked_count_skips_rejected_and_disabled_targets() {
        use vf_project::{MaskKind, Mode, Style, Target, TargetKind, Video};
        let mut p = Project::new();
        p.video = Some(Video {
            path: "x.mp4".into(),
            content_hash: "h".into(),
            duration: 10.0,
            fps: 30.0,
            width: 16,
            height: 16,
        });
        let style = Style { kind: MaskKind::Solid, strength: 1.0, padding: 0, feather: false, motion_pad: false };
        p.targets.push(Target {
            id: "t".into(),
            name: "x".into(),
            kind: TargetKind::Text,
            color: "#fff".into(),
            variants: vec![],
            pattern: Some("x".into()),
            match_mode: vf_project::TextMatchMode::Contains,
            fuzzy_edits: 1,
            engine: Engine::Ocr,
            mode: Mode::Standard,
            threshold: 0.6,
            roi: None,
            style,
            enabled: true, scan_start: 0.0, scan_end: None,
        });
        let a: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut s1 = seg_with(vec![(0.0, a)]);
        s1.status = SegmentStatus::Pending;
        let mut s2 = seg_with(vec![(0.0, a)]);
        s2.id = "s2".into();
        s2.status = SegmentStatus::Rejected;
        p.segments = vec![s1, s2];
        assert_eq!(masked_segment_count(&p, true), 1);
        assert_eq!(masked_segment_count(&p, false), 0, "待确认段在仅已确认策略下不计入");
        p.targets[0].enabled = false;
        assert_eq!(masked_segment_count(&p, true), 0);
    }

    #[test]
    fn confirmed_only_policy_requires_confirmation() {
        let mut pending = seg_with(vec![]);
        pending.status = SegmentStatus::Pending;
        assert!(should_mask(&pending, true));
        assert!(!should_mask(&pending, false));

        pending.status = SegmentStatus::Confirmed;
        assert!(should_mask(&pending, false));

        pending.status = SegmentStatus::Rejected;
        assert!(!should_mask(&pending, true));
    }
}
