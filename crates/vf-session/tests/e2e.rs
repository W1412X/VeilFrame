//! 端到端：用合成的短视频跑完整的「打开 → 扫描 → 导出」链路。
//!
//! 需要本机有 ffmpeg/ffprobe（可用 VF_FFMPEG / VF_FFPROBE 指定）；缺失时打印提示并跳过，
//! 以免在没装 ffmpeg 的机器上误报失败。CI 会安装 ffmpeg，因此这些测试会真实执行。

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use vf_export::ExportOptions;
use vf_media::stream::{FfmpegPaths, FrameStream};
use vf_media::{FfprobeBackend, MediaBackend};
use vf_project::{Engine, MaskKind, Mode, Style, Target, TargetKind, TextMatchMode};
use vf_session::{ScanOptions, Session, SessionConfig};

const W: usize = 320;
const H: usize = 240;

struct Fixture {
    dir: PathBuf,
    paths: FfmpegPaths,
    video: PathBuf,
}

fn ffmpeg_paths() -> Option<FfmpegPaths> {
    let ffmpeg = env::var_os("VF_FFMPEG").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let ffprobe = env::var_os("VF_FFPROBE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ffprobe"));
    let ok = [&ffmpeg, &ffprobe]
        .iter()
        .all(|bin| Command::new(bin).arg("-version").output().map(|o| o.status.success()).unwrap_or(false));
    ok.then_some(FfmpegPaths { ffmpeg, ffprobe })
}

fn run(bin: &Path, args: &[&str]) {
    let out = Command::new(bin).args(args).output().expect("无法启动 ffmpeg");
    assert!(
        out.status.success(),
        "命令失败：{} {:?}\n{}",
        bin.display(),
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 生成 2 秒、10 fps 的素材：深灰底 + 左上角偏移的 40×40 白色方块，内含 16×16 黑块。
/// 图案不对称，NCC 模板匹配能唯一定位。
fn fixture(name: &str) -> Option<Fixture> {
    let paths = match ffmpeg_paths() {
        Some(p) => p,
        None => {
            eprintln!("跳过端到端测试：未找到 ffmpeg/ffprobe");
            return None;
        }
    };
    let dir = env::temp_dir().join(format!("vf-session-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let video = dir.join("source.mp4");
    run(
        &paths.ffmpeg,
        &[
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=0x405060:s=320x240:r=10:d=2",
            "-vf",
            "drawbox=x=200:y=40:w=40:h=40:color=white:t=fill,drawbox=x=210:y=50:w=16:h=16:color=black:t=fill",
            "-c:v",
            "mpeg4",
            "-q:v",
            "2",
            video.to_str().unwrap(),
        ],
    );
    Some(Fixture { dir, paths, video })
}

fn config(fx: &Fixture) -> SessionConfig {
    SessionConfig {
        ffmpeg: fx.paths.clone(),
        cache_db: fx.dir.join("cache.db"),
        ..Default::default()
    }
}

fn image_target(template: &Path) -> Target {
    Target {
        id: "logo".into(),
        name: "logo".into(),
        kind: TargetKind::Image,
        color: "#2a9d8f".into(),
        variants: vec![template.to_string_lossy().into_owned()],
        pattern: None,
        match_mode: TextMatchMode::Contains,
        fuzzy_edits: 1,
        engine: Engine::Ncc,
        mode: Mode::Fast,
        threshold: 0.55,
        roi: None,
        style: Style { kind: MaskKind::Solid, strength: 1.0, padding: 0, feather: false, motion_pad: false },
        enabled: true, scan_start: 0.0, scan_end: None,
    }
}

/// 读取输出的第 `index` 帧（RGB24）。
fn read_frame(paths: &FfmpegPaths, file: &Path, index: usize) -> Vec<u8> {
    let mut stream = FrameStream::open(&paths.ffmpeg, file, None, Some((W, H))).expect("打开解码流");
    let mut last = None;
    for _ in 0..=index {
        match stream.next_frame().expect("解码失败") {
            Some(frame) => last = Some(frame),
            None => break,
        }
    }
    last.expect("输出帧数不足")
}

#[test]
fn scan_and_export_synthetic_logo() {
    let Some(fx) = fixture("logo") else { return };
    let template = fx.dir.join("template.png");
    run(
        &fx.paths.ffmpeg,
        &[
            "-v",
            "error",
            "-y",
            "-i",
            fx.video.to_str().unwrap(),
            "-frames:v",
            "1",
            "-vf",
            "crop=40:40:200:40",
            template.to_str().unwrap(),
        ],
    );

    let mut session = Session::open_video(&fx.video, config(&fx)).expect("打开视频");
    let video = session.project().video.clone().expect("视频信息");
    assert_eq!((video.width, video.height), (W as u32, H as u32));
    assert!((video.duration - 2.0).abs() < 0.2, "时长 {}", video.duration);

    session.add_target(image_target(&template)).expect("添加目标");
    let cancel = AtomicBool::new(false);
    let mut ticks = Vec::new();
    let summary = session
        .scan_targets(&["logo".into()], ScanOptions { interval: 0.5, ..Default::default() }, &cancel, |p| ticks.push(p.t))
        .expect("扫描");
    assert!(!summary.cancelled);
    assert!(!ticks.is_empty(), "应产生进度事件");
    assert!(summary.segments >= 1, "应检出合成 logo");

    let seg = &session.project().segments[0];
    assert!(seg.t0 <= 1e-9 && seg.t1 > 1.5, "静态 logo 应覆盖整段，实际 {}–{}", seg.t0, seg.t1);
    let q = seg.keyframes[0].quad;
    let cx = (q[0][0] + q[2][0]) / 2.0;
    let cy = (q[0][1] + q[2][1]) / 2.0;
    assert!((cx - 220.0).abs() < 12.0, "检出中心 x={cx}，期望 220");
    assert!((cy - 60.0).abs() < 12.0, "检出中心 y={cy}，期望 60");

    let output = fx.dir.join("masked.mp4");
    let opts = ExportOptions { ffmpeg: fx.paths.clone(), codec: "libx264".into(), crf: 20, include_pending: true, ..Default::default() };
    let mut exported = Vec::new();
    let report = session.export(&output, &opts, &cancel, |done, _| exported.push(done)).expect("导出");
    assert_eq!(report.frames, 20, "2 秒 × 10 fps");
    assert!(report.masked_regions >= 1);
    assert_eq!(exported.last().copied(), Some(report.frames));

    let probe = FfprobeBackend { bin: fx.paths.ffprobe.clone() }.probe(&output).expect("探测输出");
    assert!((probe.duration - 2.0).abs() < 0.25, "输出时长 {}", probe.duration);

    // 对比源视频与输出的同一帧：logo 区域应被明显改动，背景保持不变。
    let src = read_frame(&fx.paths, &fx.video, 10);
    let out = read_frame(&fx.paths, &output, 10);
    let changed = |x: usize, y: usize| {
        let i = (y * W + x) * 3;
        (0..3).any(|c| (src[i + c] as i32 - out[i + c] as i32).abs() > 40)
    };
    let mut masked_px = 0;
    for y in 40..80 {
        for x in 200..240 {
            masked_px += usize::from(changed(x, y));
        }
    }
    assert!(masked_px > 1600 / 2, "logo 区域应被大面积打码（{masked_px}/1600）");
    let mut background_px = 0;
    for y in 140..220 {
        for x in 20..140 {
            background_px += usize::from(changed(x, y));
        }
    }
    assert!(background_px < 9600 / 20, "背景不应被改动（{background_px}/9600）");

    std::fs::remove_dir_all(&fx.dir).unwrap();
}

#[test]
fn scan_honours_cancel_between_targets() {
    let Some(fx) = fixture("cancel") else { return };
    let mut session = Session::open_video(&fx.video, config(&fx)).expect("打开视频");
    // 固定区域不依赖模型，最适合验证取消不会继续产生新段。
    session
        .add_target(Target {
            id: "zone".into(),
            name: "zone".into(),
            kind: TargetKind::Region,
            color: "#d1495b".into(),
            variants: vec![],
            pattern: None,
            match_mode: TextMatchMode::Contains,
            fuzzy_edits: 1,
            engine: Engine::Auto,
            mode: Mode::Standard,
            threshold: 0.6,
            roi: Some([10, 10, 50, 40]),
            style: Style { kind: MaskKind::Mosaic, strength: 0.6, padding: 4, feather: true, motion_pad: false },
            enabled: true, scan_start: 0.0, scan_end: None,
        })
        .expect("添加目标");

    let cancel = AtomicBool::new(true);
    let summary = session
        .scan_targets(&["zone".into()], ScanOptions::default(), &cancel, |_| {})
        .expect("扫描");
    assert!(summary.cancelled);
    assert_eq!(summary.segments, 0);
    assert!(session.project().segments.is_empty());
    assert_eq!(cancel.load(Ordering::Relaxed), true);

    std::fs::remove_dir_all(&fx.dir).unwrap();
}
