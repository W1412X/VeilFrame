//! 导出（对应 `first_design.md` 1.6）。
//!
//! 全量模式的流水线：ffmpeg 解码（rgb24）→ 预分配缓冲池 → 渲染线程原地打码 → 按采样顺序写入 ffmpeg 编码器。
//! 缓冲区在线程间循环使用，帧数据不做额外拷贝。
//!
//! 智能渲染（源为 H.264 且编码器为 libx264 系列时默认开启）：按关键帧切分 GOP，
//! 不含打码的 GOP 直接流拷贝，含打码的 GOP 重新编码，最后拼接并拷贝源视频的音频。
//!
//! 输出先写临时文件，完成后原子改名，取消或失败时不会留下半成品文件。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use opencv::core;
use vf_media::stream::FfmpegPaths;
use vf_media::{media_command, FfprobeBackend, MediaBackend, VideoInfo};
use vf_project::{Project, Quad, Segment, SegmentStatus, Style};
use vf_render::{render_region_in, RenderError, RenderScratch};

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
    /// 打码渲染线程数，取值 1 到 32。
    pub render_threads: usize,
    /// 智能渲染：只重新编码含打码的 GOP。源不是 H.264 或编码器不是 libx264 系列时自动退回全量重编码。
    pub smart_render: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            ffmpeg: FfmpegPaths::default(),
            codec: "libx264".into(),
            crf: 18,
            include_pending: true,
            render_threads: 4,
            smart_render: true,
        }
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

/// 该段在当前导出策略下是否打码。
fn should_mask(seg: &Segment, include_pending: bool) -> bool {
    seg.status != SegmentStatus::Rejected && (include_pending || seg.status == SegmentStatus::Confirmed)
}

/// 需要打码的段及其目标样式（目标未启用或段不打码时不出现在计划里）。
fn build_plan<'a>(project: &'a Project, include_pending: bool) -> Vec<(&'a Segment, &'a Style)> {
    project
        .segments
        .iter()
        .filter(|s| should_mask(s, include_pending))
        .filter_map(|s| {
            let target = project.target(&s.target)?;
            target.enabled.then_some((s, &target.style))
        })
        .collect()
}

/// 导出一个项目。`progress` 每写出一帧调用一次，参数为（已写出帧数，总帧数）。
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
    let total = (info.duration * info.fps).round() as u64;
    let plan = build_plan(project, opts.include_pending);
    let threads = opts.render_threads.clamp(1, 32);
    // 渲染线程之间已经并行，OpenCV 内部再开线程会相乘，多线程时把它限为 1。
    let _cv_threads = (threads > 1).then(|| CvThreads::limit(1));
    let tmp = output.with_extension("exporting.mp4");
    let ctx = Ctx { opts, input: &input, info: &info, plan: &plan, threads, cancel, tmp: &tmp, total };

    let smart = opts.smart_render && is_smart_eligible(&info, &opts.codec);
    let result = if smart {
        match export_smart(&ctx, &mut progress) {
            // 智能渲染的拼接或 ffmpeg 出错时，退回全量重编码，保证结果可用。
            Err(ExportError::Ffmpeg(msg)) => {
                eprintln!("智能渲染失败，改用全量重编码：{msg}");
                export_full(&ctx, &mut progress)
            }
            other => other,
        }
    } else {
        export_full(&ctx, &mut progress)
    };

    match result {
        Ok(mut report) => {
            std::fs::rename(&tmp, output).map_err(|e| ExportError::Io(e.to_string()))?;
            report.output = output.to_path_buf();
            Ok(report)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 导出过程共享的参数。
struct Ctx<'a> {
    opts: &'a ExportOptions,
    input: &'a Path,
    info: &'a VideoInfo,
    plan: &'a [(&'a Segment, &'a Style)],
    threads: usize,
    cancel: &'a AtomicBool,
    tmp: &'a Path,
    total: u64,
}

/// 智能渲染需要源为 H.264，且编码器也是 H.264（libx264 或 h264 系列）。
fn is_smart_eligible(info: &VideoInfo, codec: &str) -> bool {
    info.codec == "h264" && (codec.starts_with("libx264") || codec.starts_with("h264"))
}

/// 全量重编码：整段解码、逐帧打码、整段编码，音频直接拷贝。
fn export_full(ctx: &Ctx, progress: &mut impl FnMut(u64, u64)) -> Result<ExportReport, ExportError> {
    let spec = StreamSpec {
        ffmpeg: ctx.opts.ffmpeg.ffmpeg.as_path(),
        input: ctx.input,
        range: None,
        width: ctx.info.width as usize,
        height: ctx.info.height as usize,
        fps: ctx.info.fps,
        t_offset: 0.0,
        codec: &ctx.opts.codec,
        crf: ctx.opts.crf,
        container: "mp4",
        with_audio: true,
        output: ctx.tmp,
    };
    let total = ctx.total;
    let r = run_stream(&spec, ctx.plan, ctx.threads, ctx.cancel, &mut |n| progress(n, total))?;
    Ok(ExportReport { frames: r.frames, masked_regions: r.masked, output: PathBuf::new() })
}

/// 单条流水线的输入输出描述。
struct StreamSpec<'a> {
    ffmpeg: &'a Path,
    input: &'a Path,
    /// 解码范围 `(起点秒, 帧数)`，`None` 表示整段。帧数由 `-frames:v` 精确限制。
    range: Option<(f64, u64)>,
    width: usize,
    height: usize,
    fps: f64,
    /// 第一帧对应的视频绝对时刻（秒）。
    t_offset: f64,
    codec: &'a str,
    crf: u8,
    /// 输出容器：`mp4` 或 `mpegts`。
    container: &'a str,
    /// 是否把源视频的音频拷贝进输出（只用于整段输出）。
    with_audio: bool,
    output: &'a Path,
}

struct StreamResult {
    frames: u64,
    masked: u64,
}

/// 解码 → 渲染 → 编码的流水线。
///
/// 解码线程从空闲池取缓冲区、读入一帧后交给渲染线程；渲染线程原地打码后交还给主线程；
/// 主线程按序号写入编码器，并把缓冲区放回空闲池。缓冲池大小固定，因此内存占用有上限。
fn run_stream(
    spec: &StreamSpec,
    plan: &[(&Segment, &Style)],
    threads: usize,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<StreamResult, ExportError> {
    let frame_len = spec.width * spec.height * 3;
    let (w, h) = (spec.width as i32, spec.height as i32);
    let fps = spec.fps;
    let t_offset = spec.t_offset;

    // 解码器
    let mut dec_args: Vec<String> = vec!["-v".to_string(), "error".to_string()];
    if let Some((start, _)) = spec.range {
        dec_args.push("-ss".to_string());
        dec_args.push(format!("{start:.6}"));
    }
    dec_args.push("-i".to_string());
    dec_args.push(spec.input.display().to_string());
    if let Some((_, frames)) = spec.range {
        dec_args.push("-frames:v".to_string());
        dec_args.push(frames.to_string());
    }
    for a in ["-an", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"] {
        dec_args.push(a.to_string());
    }
    let mut dec = media_command(spec.ffmpeg)
        .args(&dec_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ExportError::Ffmpeg(e.to_string()))?;
    let dec_out = dec.stdout.take().ok_or_else(|| ExportError::Ffmpeg("无法读取解码器输出".into()))?;

    // 编码器
    let size_s = format!("{}x{}", spec.width, spec.height);
    let fps_s = format!("{fps:.6}");
    let crf_s = spec.crf.to_string();
    let input_s = spec.input.display().to_string();
    let output_s = spec.output.display().to_string();
    let mut enc_parts: Vec<&str> =
        vec!["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", size_s.as_str(), "-r", fps_s.as_str(), "-i", "-"];
    if spec.with_audio {
        enc_parts.extend(["-i", input_s.as_str(), "-map", "0:v:0", "-map", "1:a?"]);
    } else {
        enc_parts.extend(["-map", "0:v:0"]);
    }
    enc_parts.extend(["-c:v", spec.codec, "-crf", crf_s.as_str(), "-pix_fmt", "yuv420p"]);
    if spec.with_audio {
        // 不用 -shortest：它会在管道视频的末尾截掉约 2.5 秒，导致帧数少于源视频
        enc_parts.extend(["-c:a", "copy"]);
    }
    if spec.container == "mpegts" {
        // 让 mpegts 的起始时间戳从 0 开始，片段拼接后时间轴才与源视频对齐
        enc_parts.extend(["-muxdelay", "0", "-muxpreload", "0"]);
    }
    enc_parts.extend(["-f", spec.container, output_s.as_str()]);
    let enc_args: Vec<String> = enc_parts.iter().map(|s| s.to_string()).collect();
    let mut enc = match media_command(spec.ffmpeg)
        .args(&enc_args)
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = dec.kill();
            return Err(ExportError::Ffmpeg(e.to_string()));
        }
    };
    let mut enc_in: ChildStdin = enc.stdin.take().ok_or_else(|| ExportError::Ffmpeg("无法写入编码器".into()))?;

    // 缓冲池与通道
    let pool = threads * 2 + 2;
    let (free_tx, free_rx) = mpsc::channel::<Vec<u8>>();
    for _ in 0..pool {
        let _ = free_tx.send(vec![0u8; frame_len]);
    }
    let (work_tx, work_rx) = mpsc::channel::<(u64, Vec<u8>)>();
    let work_rx = Mutex::new(work_rx);
    let (done_tx, done_rx) = mpsc::channel::<Result<(u64, Vec<u8>, u64), String>>();

    let mut next: u64 = 0;
    let mut masked_total: u64 = 0;
    let mut failure: Option<ExportError> = None;

    thread::scope(|scope| {
        // 解码线程：读入一帧后交给渲染线程。源结束、出错或通道关闭时退出。
        scope.spawn(move || {
            let mut src = dec_out;
            let mut idx: u64 = 0;
            while let Ok(mut buf) = free_rx.recv() {
                match read_full(&mut src, &mut buf) {
                    Ok(true) => {}
                    _ => break,
                }
                if work_tx.send((idx, buf)).is_err() {
                    break;
                }
                idx += 1;
            }
        });

        // 渲染线程：每个线程有自己的 RenderScratch，缓冲区就地修改。
        let work_rx = &work_rx;
        for _ in 0..threads {
            let done_tx = done_tx.clone();
            scope.spawn(move || {
                let mut scratch = RenderScratch::default();
                loop {
                    let job = match work_rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => break,
                    };
                    let Ok((idx, mut buf)) = job else { break };
                    let t = t_offset + idx as f64 / fps;
                    let msg = render_frame(&mut scratch, &mut buf, w, h, t, fps, plan).map(|n| (idx, buf, n));
                    let failed = msg.is_err();
                    if done_tx.send(msg).is_err() || failed {
                        break;
                    }
                }
            });
        }
        drop(done_tx);

        // 主线程：按序号写入编码器，并把缓冲区放回空闲池。
        let mut pending: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                failure = Some(ExportError::Cancelled);
                break;
            }
            while let Some(buf) = pending.remove(&next) {
                if let Err(e) = enc_in.write_all(&buf) {
                    failure = Some(ExportError::Ffmpeg(format!("写入编码器失败：{e}")));
                    break;
                }
                let _ = free_tx.send(buf);
                next += 1;
                progress(next);
            }
            if failure.is_some() {
                break;
            }
            match done_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok((idx, buf, n))) => {
                    masked_total += n;
                    pending.insert(idx, buf);
                }
                Ok(Err(msg)) => {
                    failure = Some(ExportError::Render(RenderError::Cv(msg)));
                    break;
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        // 收尾：关闭空闲池让解码线程退出，杀掉解码器解除阻塞，等待渲染线程结束。
        drop(free_tx);
        let _ = dec.kill();
        while done_rx.recv().is_ok() {}
        drop(enc_in);
    });

    let _ = dec.wait();
    let enc_status = enc.wait().map_err(|e| ExportError::Ffmpeg(e.to_string()))?;
    if let Some(e) = failure {
        return Err(e);
    }
    if !enc_status.success() {
        return Err(ExportError::Ffmpeg(format!("编码器退出码 {:?}", enc_status.code())));
    }
    Ok(StreamResult { frames: next, masked: masked_total })
}

/// 对一帧（RGB24，原地）应用计划中所有处于活动状态的打码项，返回实际作用的次数。
fn render_frame(
    scratch: &mut RenderScratch,
    buf: &mut [u8],
    w: i32,
    h: i32,
    t: f64,
    fps: f64,
    plan: &[(&Segment, &Style)],
) -> Result<u64, String> {
    let mut mat = core::Mat::new_rows_cols_with_bytes_mut::<core::Vec3b>(h, w, buf).map_err(|e| e.to_string())?;
    let mut applied = 0u64;
    for &(seg, style) in plan {
        if t < seg.t0 || t > seg.t1 {
            continue;
        }
        let (quad, speed) = quad_at(seg, t, fps);
        if render_region_in(scratch, &mut mat, &quad, style, speed).map_err(|e| e.to_string())? {
            applied += 1;
        }
    }
    Ok(applied)
}

/// 读满一帧。返回 `Ok(false)` 表示源已结束（不足一帧）。
fn read_full(src: &mut impl Read, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = src.read(&mut buf[filled..])?;
        if n == 0 {
            return Ok(false);
        }
        filled += n;
    }
    Ok(true)
}

/// 智能渲染：按关键帧切 GOP，未打码的 GOP 流拷贝，打码的 GOP 重编码，最后拼接并拷贝音频。
fn export_smart(ctx: &Ctx, progress: &mut impl FnMut(u64, u64)) -> Result<ExportReport, ExportError> {
    let ffprobe = ctx.opts.ffmpeg.ffprobe.as_path();
    let (keys, _video_end) = probe_timeline(ffprobe, ctx.input, ctx.info.fps)?;
    let windows: Vec<(f64, f64)> = keys
        .windows(2)
        .map(|w| (w[0], w[1]))
        .filter(|&(a, b)| b > a + 1e-9)
        .collect();
    let runs = group_runs(&windows, ctx.plan);

    let work = work_dir(ctx.tmp);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| ExportError::Io(e.to_string()))?;
    let result = smart_parts(ctx, &runs, &work, progress).and_then(|(parts, frames, masked)| {
        let list = work.join("list.txt");
        let mut text = String::new();
        for p in &parts {
            // 列表文件与片段在同一目录，只写文件名，避免相对路径问题
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            text.push_str(&format!("file '{name}'\n"));
        }
        std::fs::write(&list, text).map_err(|e| ExportError::Io(e.to_string()))?;
        // 不加时间戳修正：mp4 封装器会把起点对齐到 0，与整段输出的音视频起点一致（已用 ffprobe 验证）。
        let status = media_command(ctx.opts.ffmpeg.ffmpeg.as_path())
            .args(["-v", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
            .arg(&list)
            .arg("-i")
            .arg(ctx.input)
            .args(["-map", "0:v:0", "-map", "1:a?", "-c", "copy", "-f", "mp4"])
            .arg(ctx.tmp)
            .status()
            .map_err(|e| ExportError::Ffmpeg(e.to_string()))?;
        if !status.success() {
            return Err(ExportError::Ffmpeg("拼接片段失败".into()));
        }
        Ok(ExportReport { frames, masked_regions: masked, output: PathBuf::new() })
    });
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// 逐段生成片段文件，返回片段列表、总帧数与打码次数。
fn smart_parts(
    ctx: &Ctx,
    runs: &[(f64, f64, bool)],
    work: &Path,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(Vec<PathBuf>, u64, u64), ExportError> {
    let fps = ctx.info.fps;
    let total = ctx.total;
    let ffmpeg = ctx.opts.ffmpeg.ffmpeg.as_path();
    let mut parts = Vec::with_capacity(runs.len());
    let mut frames = 0u64;
    let mut masked = 0u64;
    for (i, &(start, end, is_masked)) in runs.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        let part = work.join(format!("part{i:05}.ts"));
        // 片段帧数由关键帧时刻决定，与源视频的帧网格一致
        let f0 = (start * fps).round() as u64;
        let f1 = (end * fps).round() as u64;
        if is_masked {
            let spec = StreamSpec {
                ffmpeg,
                input: ctx.input,
                range: Some((start, f1.saturating_sub(f0))),
                width: ctx.info.width as usize,
                height: ctx.info.height as usize,
                fps,
                t_offset: start,
                codec: &ctx.opts.codec,
                crf: ctx.opts.crf,
                container: "mpegts",
                with_audio: false,
                output: &part,
            };
            let base = frames;
            let r = run_stream(&spec, ctx.plan, ctx.threads, ctx.cancel, &mut |n| progress(base + n, total))?;
            frames += r.frames;
            masked += r.masked;
        } else {
            // 流拷贝：起点是关键帧，片段可以独立解码。帧数用 -frames:v 精确限制
            // （-t 的边界会包含下一个关键帧，多拷贝约 2 帧）。
            let args: Vec<String> = vec![
                "-v".to_string(),
                "error".to_string(),
                "-y".to_string(),
                "-ss".to_string(),
                format!("{start:.6}"),
                "-i".to_string(),
                ctx.input.display().to_string(),
                "-frames:v".to_string(),
                (f1.saturating_sub(f0)).to_string(),
                "-map".to_string(),
                "0:v:0".to_string(),
                "-an".to_string(),
                "-c:v".to_string(),
                "copy".to_string(),
                "-muxdelay".to_string(),
                "0".to_string(),
                "-muxpreload".to_string(),
                "0".to_string(),
                "-f".to_string(),
                "mpegts".to_string(),
                part.display().to_string(),
            ];
            let status = media_command(ffmpeg)
                .args(&args)
                .status()
                .map_err(|e| ExportError::Ffmpeg(e.to_string()))?;
            if !status.success() {
                return Err(ExportError::Ffmpeg(format!("拷贝片段 {start:.2}s 失败")));
            }
            frames += f1.saturating_sub(f0);
            progress(frames, total);
        }
        parts.push(part);
    }
    Ok((parts, frames, masked))
}

/// 探测视频流的时间线：关键帧时刻（秒，含 0）与视频结束时刻。
///
/// 结束时刻取最后一个视频包的时间戳加一帧，而不是容器时长：容器时长可能包含更长的音频，
/// 用它会多出几帧。
fn probe_timeline(ffprobe: &Path, input: &Path, fps: f64) -> Result<(Vec<f64>, f64), ExportError> {
    let out = media_command(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0"])
        .arg(input)
        .output()
        .map_err(|e| ExportError::Probe(e.to_string()))?;
    if !out.status.success() {
        return Err(ExportError::Probe("读取视频包失败".into()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut all: Vec<(f64, bool)> = text
        .lines()
        .filter_map(|line| {
            let mut it = line.split(',');
            let pts = it.next()?.trim().parse::<f64>().ok()?;
            let flags = it.next()?;
            Some((pts, flags.contains('K')))
        })
        .collect();
    if all.is_empty() {
        return Err(ExportError::Probe("视频没有可用的包".into()));
    }
    let last = all.iter().map(|&(t, _)| t).fold(f64::MIN, f64::max);
    let end = last + 1.0 / fps;
    let mut keys: Vec<f64> = all.drain(..).filter(|&(t, k)| k && t < end - 1e-6).map(|(t, _)| t).collect();
    keys.sort_by(|a, b| a.total_cmp(b));
    keys.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    if keys.first().is_none_or(|&t| t > 1e-6) {
        keys.insert(0, 0.0);
    }
    keys.push(end);
    Ok((keys, end))
}

/// 合并相邻的 GOP：同为打码或同为不打码的连续 GOP 合成一段。
/// 一个 GOP 只要与任何打码段的时间区间相交，就视为需要重编码。
fn group_runs(windows: &[(f64, f64)], plan: &[(&Segment, &Style)]) -> Vec<(f64, f64, bool)> {
    let mut runs: Vec<(f64, f64, bool)> = Vec::new();
    for &(g0, g1) in windows {
        let masked = plan.iter().any(|&(s, _)| s.t0 < g1 && s.t1 >= g0);
        match runs.last_mut() {
            Some(last) if last.2 == masked => last.1 = g1,
            _ => runs.push((g0, g1, masked)),
        }
    }
    runs
}

/// 智能渲染的临时目录，与输出文件同目录。
fn work_dir(tmp: &Path) -> PathBuf {
    let parent = tmp.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let stem = tmp.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    parent.join(format!(".vf-smart-{stem}"))
}

/// 限制 OpenCV 线程数，析构时恢复原值。
struct CvThreads(i32);

impl CvThreads {
    fn limit(n: i32) -> Self {
        let prev = core::get_num_threads().unwrap_or(-1);
        let _ = core::set_num_threads(n);
        Self(prev)
    }
}

impl Drop for CvThreads {
    fn drop(&mut self) {
        let _ = core::set_num_threads(self.0);
    }
}

/// 该段在当前导出策略下打码的条数（用于界面提示）。
pub fn masked_segment_count(project: &Project, include_pending: bool) -> usize {
    project
        .segments
        .iter()
        .filter(|s| should_mask(s, include_pending))
        .filter(|s| project.target(&s.target).is_some_and(|t| t.enabled))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_project::{Engine, Keyframe, MaskKind, Mode, Source, Target, TargetKind, TextMatchMode, Video};

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
            match_mode: TextMatchMode::Contains,
            fuzzy_edits: 1,
            engine: Engine::Ocr,
            mode: Mode::Standard,
            threshold: 0.6,
            roi: None,
            style,
            enabled: true,
            scan_start: 0.0,
            scan_end: None,
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

    #[test]
    fn gop_runs_merge_neighbours_with_same_flag() {
        let windows = vec![(0.0, 2.0), (2.0, 4.0), (4.0, 6.0), (6.0, 8.0)];
        let mut seg = seg_with(vec![]);
        seg.t0 = 3.0;
        seg.t1 = 5.0;
        let style = Style { kind: MaskKind::Solid, strength: 1.0, padding: 0, feather: false, motion_pad: false };
        let plan = vec![(&seg, &style)];
        let runs = group_runs(&windows, &plan);
        assert_eq!(runs, vec![(0.0, 2.0, false), (2.0, 6.0, true), (6.0, 8.0, false)]);
    }

    #[test]
    fn gop_runs_without_masks_form_one_copy_run() {
        let windows = vec![(0.0, 2.0), (2.0, 4.0)];
        let runs = group_runs(&windows, &[]);
        assert_eq!(runs, vec![(0.0, 4.0, false)]);
    }

    #[test]
    fn smart_render_only_for_h264_sources_and_h264_encoders() {
        let info = VideoInfo { duration: 1.0, fps: 30.0, width: 16, height: 16, codec: "h264".into() };
        assert!(is_smart_eligible(&info, "libx264"));
        assert!(is_smart_eligible(&info, "h264_nvenc"));
        let hevc = VideoInfo { codec: "hevc".into(), ..info };
        assert!(!is_smart_eligible(&hevc, "libx264"));
    }
}
