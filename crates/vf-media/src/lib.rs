//! 媒体 I/O 抽象（ADR-0002）。第一期通过 ffprobe/ffmpeg 命令行实现，
//! 进程隔离，便于替换为 libav 绑定。

pub mod hash;
pub mod stream;

pub use hash::sampled_content_hash;

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 视频基础元数据。
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub duration: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub codec: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("无法启动 {bin}：{reason}")]
    Spawn { bin: String, reason: String },
    #[error("ffprobe 执行失败：{0}")]
    ProbeFailed(String),
    #[error("ffprobe 输出无法解析：{0}")]
    Parse(String),
    #[error("文件中没有视频流")]
    NoVideoStream,
    #[error("抽帧数据长度不符：期望 {expected} 字节，实际 {got} 字节")]
    FrameSize { expected: usize, got: usize },
}

/// 媒体后端接口。上层只依赖此 trait。
pub trait MediaBackend {
    fn probe(&self, path: &Path) -> Result<VideoInfo, MediaError>;
}

#[derive(Debug, Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

/// 解析形如 `30000/1001` 或 `25` 的帧率字符串。
pub fn parse_rate(s: &str) -> Option<f64> {
    match s.split_once('/') {
        Some((n, d)) => {
            let n: f64 = n.trim().parse().ok()?;
            let d: f64 = d.trim().parse().ok()?;
            if d == 0.0 || n == 0.0 {
                None
            } else {
                Some(n / d)
            }
        }
        None => s.trim().parse().ok().filter(|v: &f64| *v > 0.0),
    }
}

/// 解析 ffprobe 的 JSON 输出（`-print_format json -show_format -show_streams`）。
pub fn parse_ffprobe_json(json: &str) -> Result<VideoInfo, MediaError> {
    let out: ProbeOutput = serde_json::from_str(json).map_err(|e| MediaError::Parse(e.to_string()))?;
    let video = out
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .ok_or(MediaError::NoVideoStream)?;

    let fps = video
        .avg_frame_rate
        .as_deref()
        .and_then(parse_rate)
        .or_else(|| video.r_frame_rate.as_deref().and_then(parse_rate))
        .unwrap_or(0.0);

    let duration = out
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .or(video.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);

    Ok(VideoInfo {
        duration,
        fps,
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        codec: video.codec_name.clone().unwrap_or_default(),
    })
}

/// 通过 ffprobe 命令行探测。`bin` 默认为 `ffprobe`（需在 PATH 中，或指定绝对路径）。
#[derive(Debug, Clone)]
pub struct FfprobeBackend {
    pub bin: PathBuf,
}

impl Default for FfprobeBackend {
    fn default() -> Self {
        Self { bin: PathBuf::from("ffprobe") }
    }
}

impl MediaBackend for FfprobeBackend {
    fn probe(&self, path: &Path) -> Result<VideoInfo, MediaError> {
        let output = Command::new(&self.bin)
            .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
            .arg(path)
            .output()
            .map_err(|e| MediaError::Spawn {
                bin: self.bin.display().to_string(),
                reason: e.to_string(),
            })?;
        if !output.status.success() {
            return Err(MediaError::ProbeFailed(String::from_utf8_lossy(&output.stderr).trim().to_string()));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        parse_ffprobe_json(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "streams": [
            {"codec_type": "audio", "codec_name": "aac"},
            {"codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080,
             "avg_frame_rate": "30000/1001", "r_frame_rate": "30000/1001"}
        ],
        "format": {"duration": "225.500000"}
    }"#;

    #[test]
    fn parses_fractional_frame_rate_and_duration() {
        let info = parse_ffprobe_json(SAMPLE).unwrap();
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert_eq!(info.codec, "h264");
        assert!((info.fps - 29.97002997).abs() < 1e-6);
        assert!((info.duration - 225.5).abs() < 1e-9);
    }

    #[test]
    fn falls_back_to_stream_duration() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"hevc","width":640,"height":360,
            "avg_frame_rate":"25/1","duration":"10.0"}],"format":{}}"#;
        let info = parse_ffprobe_json(json).unwrap();
        assert!((info.duration - 10.0).abs() < 1e-9);
        assert!((info.fps - 25.0).abs() < 1e-9);
    }

    #[test]
    fn audio_only_is_an_error() {
        let json = r#"{"streams":[{"codec_type":"audio"}],"format":{"duration":"1"}}"#;
        assert!(matches!(parse_ffprobe_json(json), Err(MediaError::NoVideoStream)));
    }

    #[test]
    fn rejects_zero_denominator_rate() {
        assert_eq!(parse_rate("0/0"), None);
        assert_eq!(parse_rate("30/0"), None);
    }

    #[test]
    fn missing_ffprobe_reports_spawn_error() {
        let b = FfprobeBackend { bin: PathBuf::from("definitely-not-a-binary-vf") };
        assert!(matches!(b.probe(Path::new("x.mp4")), Err(MediaError::Spawn { .. })));
    }
}

/// 抽取单帧为原始 RGB24 的请求。输出尺寸固定为代理分辨率。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRequest {
    pub t: f64,
    pub width: u32,
    pub height: u32,
}

/// 构造 ffmpeg 抽帧参数。`-ss` 放在 `-i` 之前以启用快速定位。
pub fn frame_extract_args(input: &Path, req: FrameRequest) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-ss".into(),
        format!("{:.3}", req.t),
        "-i".into(),
        input.display().to_string(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        format!("scale={}:{}", req.width, req.height),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgb24".into(),
        "-".into(),
    ]
}

/// 通过 ffmpeg 抽取一帧，返回长度为 `width * height * 3` 的 RGB 数据。
pub fn extract_frame_rgb(ffmpeg: &Path, input: &Path, req: FrameRequest) -> Result<Vec<u8>, MediaError> {
    let output = Command::new(ffmpeg)
        .args(frame_extract_args(input, req))
        .output()
        .map_err(|e| MediaError::Spawn { bin: ffmpeg.display().to_string(), reason: e.to_string() })?;
    if !output.status.success() {
        return Err(MediaError::ProbeFailed(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }
    let expected = req.width as usize * req.height as usize * 3;
    if output.stdout.len() != expected {
        return Err(MediaError::FrameSize { expected, got: output.stdout.len() });
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn seek_comes_before_input_and_size_is_scaled() {
        let args = frame_extract_args(Path::new("in.mp4"), FrameRequest { t: 12.5, width: 960, height: 540 });
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < i);
        assert_eq!(args[ss + 1], "12.500");
        assert!(args.contains(&"scale=960:540".to_string()));
        assert!(args.contains(&"rgb24".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }
}
