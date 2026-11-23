//! 顺序抽帧流：一个 ffmpeg 进程按采样间隔解码，输出原始 RGB24，逐帧读取。
//! 避免为每一帧单独启动 ffmpeg（每次约几十毫秒的进程开销）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Stdio};

use crate::{MediaBackend, MediaError};

pub struct FrameStream {
    child: Child,
    stdout: ChildStdout,
    width: usize,
    height: usize,
}

impl FrameStream {
    /// 打开视频。`fps` 为采样频率（如 `Some(2.0)` 表示每秒 2 帧），`None` 表示原始帧率。
    /// `(width, height)` 为输出尺寸（分析用的代理分辨率），`None` 表示原始尺寸。
    pub fn open(
        ffmpeg: &Path,
        input: &Path,
        fps: Option<f64>,
        size: Option<(usize, usize)>,
    ) -> Result<Self, MediaError> {
        Self::open_range(ffmpeg, input, fps, size, None)
    }

    /// 同 [`FrameStream::open`]，但只解码 `range = (起点秒, 时长秒)` 内的画面。
    /// 输出的第一帧对应起点时刻。
    pub fn open_range(
        ffmpeg: &Path,
        input: &Path,
        fps: Option<f64>,
        size: Option<(usize, usize)>,
        range: Option<(f64, f64)>,
    ) -> Result<Self, MediaError> {
        let (width, height) = match size {
            Some(s) => s,
            None => {
                let info = crate::FfprobeBackend::default().probe(input)?;
                (info.width as usize, info.height as usize)
            }
        };
        let mut filters: Vec<String> = Vec::new();
        if let Some(f) = fps {
            filters.push(format!("fps={f:.6}"));
        }
        if size.is_some() {
            filters.push(format!("scale={width}:{height}"));
        }
        let mut cmd = crate::media_command(ffmpeg);
        cmd.args(["-v", "error"]);
        if let Some((start, len)) = range {
            // -ss / -t 放在 -i 之前：输入级定位，输出时间戳从 0 开始
            let ss = format!("{start:.6}");
            let t = format!("{len:.6}");
            cmd.args(["-ss", ss.as_str(), "-t", t.as_str()]);
        }
        cmd.arg("-i").arg(input);
        if !filters.is_empty() {
            cmd.args(["-vf", &filters.join(",")]);
        }
        cmd.args(["-an", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| MediaError::Spawn {
            bin: ffmpeg.display().to_string(),
            reason: e.to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| MediaError::ProbeFailed("无法读取 ffmpeg 输出".into()))?;
        Ok(Self { child, stdout, width, height })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// 读取下一帧的 RGB24 数据。到达末尾返回 `None`。
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, MediaError> {
        let len = self.width * self.height * 3;
        let mut buf = vec![0u8; len];
        let mut filled = 0usize;
        while filled < len {
            let n = self.stdout.read(&mut buf[filled..]).map_err(|e| MediaError::ProbeFailed(e.to_string()))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        match filled {
            0 => Ok(None),
            n if n == len => Ok(Some(buf)),
            n => Err(MediaError::FrameSize { expected: len, got: n }),
        }
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 解码器与编码器的 ffmpeg 可执行文件路径。
#[derive(Debug, Clone)]
pub struct FfmpegPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Default for FfmpegPaths {
    fn default() -> Self {
        Self { ffmpeg: PathBuf::from("ffmpeg"), ffprobe: PathBuf::from("ffprobe") }
    }
}
