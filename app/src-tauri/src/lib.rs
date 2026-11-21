//! VeilFrame 桌面外壳：后台持有唯一会话（`vf-session`），命令与事件驱动界面。
//!
//! 扫描与导出在阻塞线程里执行：扫描时把 `Session` 暂移出互斥锁，结束后放回，
//! 期间查询命令返回“忙”；导出只克隆项目快照，会话仍可读取。

mod batch;

use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use vf_export::ExportOptions;
use vf_media::{media_command, FfprobeBackend, MediaBackend};
use vf_project::{Project, Quad, Target};
use vf_session::{ScanOptions, ScanProgress, ScanSummary, Session, SessionConfig};

use batch::{BatchManager, JobView, SourceInput};

#[derive(Serialize)]
pub struct ToolStatus {
    /// ffmpeg 版本首行；未安装时为 None。
    pub ffmpeg: Option<String>,
    pub ffprobe: Option<String>,
    /// ONNX Runtime 动态库是否就位。
    pub ort: bool,
    /// PP-OCRv5 检测、识别模型与字典是否就位。
    pub ocr_models: bool,
}

/// 界面读取的完整状态快照。所有编辑命令都返回它，前端不维护第二份数据。
#[derive(Serialize)]
pub struct Snapshot {
    pub project: Project,
    pub project_path: Option<String>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub autosave_error: Option<String>,
    pub scanning: bool,
    pub exporting: bool,
}

#[derive(Serialize)]
struct ScanDone {
    snapshot: Snapshot,
    summary: ScanSummary,
}

#[derive(Serialize, Clone)]
struct ExportProgress {
    done: u64,
    total: u64,
}

#[derive(Serialize)]
struct ExportDone {
    frames: u64,
    masked_regions: u64,
    output: String,
}

#[derive(Default)]
struct AppState {
    /// 扫描/导出期间为 None。
    session: Mutex<Option<Session>>,
    scanning: AtomicBool,
    exporting: AtomicBool,
    scan_cancel: AtomicBool,
    export_cancel: AtomicBool,
    batch: Arc<BatchManager>,
}

impl AppState {
    fn busy(&self) -> Option<&'static str> {
        if self.scanning.load(Ordering::SeqCst) {
            Some("正在扫描，请稍候")
        } else if self.exporting.load(Ordering::SeqCst) {
            Some("正在导出，请稍候")
        } else {
            None
        }
    }
}

fn snapshot_from(session: &Session, state: &AppState) -> Snapshot {
    Snapshot {
        project: session.project().clone(),
        project_path: session.project_path().map(|p| p.display().to_string()),
        can_undo: session.can_undo(),
        can_redo: session.can_redo(),
        autosave_error: session.last_autosave_error().map(str::to_string),
        scanning: state.scanning.load(Ordering::SeqCst),
        exporting: state.exporting.load(Ordering::SeqCst),
    }
}

fn snapshot_of(state: &AppState) -> Result<Snapshot, String> {
    let guard = state.session.lock().map_err(|_| "会话锁已损坏")?;
    match guard.as_ref() {
        Some(s) => Ok(snapshot_from(s, state)),
        None => Err(state.busy().unwrap_or("没有打开的项目").to_string()),
    }
}

/// 所有修改类命令的统一入口：忙时拒绝，空会话报错，成功后返回新快照。
fn edit(state: &AppState, f: impl FnOnce(&mut Session) -> Result<(), vf_session::SessionError>) -> Result<Snapshot, String> {
    if let Some(msg) = state.busy() {
        return Err(msg.to_string());
    }
    let mut guard = state.session.lock().map_err(|_| "会话锁已损坏")?;
    let session = guard.as_mut().ok_or("没有打开的项目")?;
    f(session).map_err(|e| e.to_string())?;
    Ok(snapshot_from(session, state))
}

fn version_line(bin: &Path) -> Option<String> {
    let out = media_command(bin).arg("-version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()?.lines().next().map(str::to_string)
}

fn ort_lib_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    }
}

fn exe_name(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// 打包后优先使用随包资源；环境变量仍可覆盖。
fn app_config(app: &AppHandle) -> SessionConfig {
    let mut cfg = SessionConfig::default();
    let Ok(res) = app.path().resource_dir() else { return cfg };

    if env::var_os("VF_FFMPEG").is_none() {
        let ffmpeg = res.join("bin").join(exe_name("ffmpeg"));
        let ffprobe = res.join("bin").join(exe_name("ffprobe"));
        if ffmpeg.is_file() && ffprobe.is_file() {
            cfg.ffmpeg.ffmpeg = ffmpeg;
            cfg.ffmpeg.ffprobe = ffprobe;
        }
    }

    if env::var_os("VF_ORT_LIB").is_none() && env::var_os("ORT_DYLIB_PATH").is_none() {
        let lib = res.join("onnxruntime").join(ort_lib_name());
        if lib.is_file() {
            cfg.ocr.ort_lib = lib;
        }
    }

    if env::var_os("VF_MODELS_DIR").is_none() {
        let models = res.join("models");
        let det = models.join("det_v5.onnx");
        let rec = models.join("rec_v5.onnx");
        let dict = models.join("ppocr_keys_v5_onnx.txt");
        if det.is_file() && rec.is_file() && dict.is_file() {
            cfg.ocr.det = det;
            cfg.ocr.rec = rec;
            cfg.ocr.dict = dict;
        }
    }
    cfg
}

/// 让 webview 的 `<video>` 能通过 asset 协议读取该文件。
fn allow_video(app: &AppHandle, path: &Path) {
    if let Err(e) = app.asset_protocol_scope().allow_file(path) {
        eprintln!("放行视频路径失败（{}）：{e}", path.display());
    }
}

/// 检查 ffmpeg / ffprobe、ONNX Runtime 与 OCR 模型是否可用。
#[tauri::command]
fn tool_status(app: AppHandle) -> ToolStatus {
    let cfg = app_config(&app);
    ToolStatus {
        ffmpeg: version_line(&cfg.ffmpeg.ffmpeg),
        ffprobe: version_line(&cfg.ffmpeg.ffprobe),
        ort: cfg.ocr.ort_lib.is_file(),
        ocr_models: cfg.ocr.det.is_file() && cfg.ocr.rec.is_file() && cfg.ocr.dict.is_file(),
    }
}

#[tauri::command]
fn snapshot(state: State<'_, AppState>) -> Result<Snapshot, String> {
    snapshot_of(&*state)
}

/// 打开视频并创建空项目（ffprobe 探测 + 抽样内容哈希）。
#[tauri::command]
fn open_video(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<Snapshot, String> {
    if let Some(msg) = state.busy() {
        return Err(msg.to_string());
    }
    let session = Session::open_video(Path::new(&path), app_config(&app)).map_err(|e| e.to_string())?;
    allow_video(&app, Path::new(&path));
    let mut guard = state.session.lock().map_err(|_| "会话锁已损坏")?;
    *guard = Some(session);
    Ok(snapshot_from(guard.as_ref().expect("刚刚写入"), &*state))
}

#[tauri::command]
fn open_project(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<Snapshot, String> {
    if let Some(msg) = state.busy() {
        return Err(msg.to_string());
    }
    let session = Session::open_project(Path::new(&path), app_config(&app)).map_err(|e| e.to_string())?;
    if let Some(video) = session.project().video.as_ref() {
        allow_video(&app, Path::new(&video.path));
    }
    let mut guard = state.session.lock().map_err(|_| "会话锁已损坏")?;
    *guard = Some(session);
    Ok(snapshot_from(guard.as_ref().expect("刚刚写入"), &*state))
}

/// 保存项目；`path` 为空时写回已有关联路径。
#[tauri::command]
fn save_project(state: State<'_, AppState>, path: Option<String>) -> Result<Snapshot, String> {
    edit(&*state, |s| s.save(path.as_deref().map(Path::new)).map(|_| ()))
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> Result<Snapshot, String> {
    edit(&*state, |s| s.undo())
}

#[tauri::command]
fn redo(state: State<'_, AppState>) -> Result<Snapshot, String> {
    edit(&*state, |s| s.redo())
}

#[tauri::command]
fn add_target(state: State<'_, AppState>, target: Target) -> Result<Snapshot, String> {
    edit(&*state, |s| s.add_target(target))
}

#[tauri::command]
fn update_target(state: State<'_, AppState>, target: Target) -> Result<Snapshot, String> {
    edit(&*state, |s| s.update_target(target))
}

#[tauri::command]
fn accept_segment(state: State<'_, AppState>, segment_id: String) -> Result<Snapshot, String> {
    edit(&*state, |s| s.accept(&segment_id))
}

#[tauri::command]
fn reject_segment(state: State<'_, AppState>, segment_id: String, negative: bool) -> Result<Snapshot, String> {
    edit(&*state, |s| s.reject(&segment_id, negative))
}

#[tauri::command]
fn delete_segment(state: State<'_, AppState>, segment_id: String) -> Result<Snapshot, String> {
    edit(&*state, |s| s.delete_segment(&segment_id))
}

#[tauri::command]
fn extend_track(state: State<'_, AppState>, segment_id: String) -> Result<Snapshot, String> {
    edit(&*state, |s| s.extend_track(&segment_id))
}

#[tauri::command]
fn set_keyframe(state: State<'_, AppState>, segment_id: String, t: f64, quad: Quad) -> Result<Snapshot, String> {
    edit(&*state, |s| s.set_keyframe(&segment_id, t, quad))
}

#[tauri::command]
fn add_manual_segment(
    state: State<'_, AppState>,
    id: String,
    target_id: String,
    t0: f64,
    t1: f64,
    quad: Quad,
) -> Result<Snapshot, String> {
    edit(&*state, |s| s.add_manual_segment(&id, &target_id, t0, t1, quad))
}

/// 扫描目标。进度通过 `vf-scan-progress` 推送；完成后返回新快照与汇总。
#[tauri::command]
async fn scan_targets(
    app: AppHandle,
    state: State<'_, AppState>,
    target_ids: Vec<String>,
    interval: Option<f64>,
) -> Result<ScanDone, String> {
    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("正在扫描".into());
    }
    if state.exporting.load(Ordering::SeqCst) {
        state.scanning.store(false, Ordering::SeqCst);
        return Err("正在导出".into());
    }
    if target_ids.is_empty() {
        state.scanning.store(false, Ordering::SeqCst);
        return Err("没有选中目标".into());
    }
    let Some(session) = state.session.lock().map_err(|_| "会话锁已损坏")?.take() else {
        state.scanning.store(false, Ordering::SeqCst);
        return Err("没有打开的项目".into());
    };
    state.scan_cancel.store(false, Ordering::SeqCst);
    let opts = ScanOptions { interval: interval.unwrap_or(0.5) };
    let app_task = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let shared = app_task.state::<AppState>();
        let mut session = session;
        let result = session.scan_targets(&target_ids, opts, &shared.scan_cancel, |p: ScanProgress| {
            let _ = app_task.emit("vf-scan-progress", p);
        });
        (session, result)
    })
    .await;

    let result = match joined {
        Ok((session, result)) => {
            *state.session.lock().map_err(|_| "会话锁已损坏")? = Some(session);
            result
        }
        Err(e) => Err(vf_session::SessionError::Scan(format!("扫描线程失败：{e}"))),
    };
    state.scanning.store(false, Ordering::SeqCst);
    let summary = result.map_err(|e| e.to_string())?;
    let snapshot = snapshot_of(&*state)?;
    Ok(ScanDone { snapshot, summary })
}

#[tauri::command]
fn cancel_scan(state: State<'_, AppState>) {
    state.scan_cancel.store(true, Ordering::SeqCst);
}

/// 导出当前项目。进度通过 `vf-export-progress` 推送（每 200 ms 一次）。
#[tauri::command]
async fn export_video(
    app: AppHandle,
    state: State<'_, AppState>,
    output: String,
    codec: Option<String>,
    crf: Option<u8>,
    include_pending: Option<bool>,
    queue_job: Option<String>,
) -> Result<ExportDone, String> {
    if state.scanning.load(Ordering::SeqCst) {
        return Err("正在扫描".into());
    }
    if state.exporting.swap(true, Ordering::SeqCst) {
        return Err("正在导出".into());
    }
    let (project, ffmpeg) = {
        let guard = state.session.lock().map_err(|_| "会话锁已损坏")?;
        let session = guard.as_ref().ok_or("没有打开的项目")?;
        (session.project().clone(), session.config().ffmpeg.clone())
    };
    state.export_cancel.store(false, Ordering::SeqCst);
    let opts = ExportOptions {
        ffmpeg,
        codec: codec.unwrap_or_else(|| "libx264".into()),
        crf: crf.unwrap_or(18),
        include_pending: include_pending.unwrap_or(true),
    };
    let app_task = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let shared = app_task.state::<AppState>();
        let mut last = Instant::now() - Duration::from_secs(1);
        vf_export::export(&project, Path::new(&output), &opts, &shared.export_cancel, |done, total| {
            if done == total || last.elapsed() >= Duration::from_millis(200) {
                last = Instant::now();
                let _ = app_task.emit("vf-export-progress", ExportProgress { done, total });
            }
        })
    })
    .await;
    state.exporting.store(false, Ordering::SeqCst);
    let report = joined
        .map_err(|e| format!("导出线程失败：{e}"))?
        .map_err(|e| e.to_string())?;
    // 从批量队列载入的项目导出成功后，任务才算真正完成。
    if let Some(id) = queue_job.as_deref() {
        state.batch.mark_done(id);
        emit_queue(&app, &*state);
    }
    Ok(ExportDone {
        frames: report.frames,
        masked_regions: report.masked_regions,
        output: report.output.display().to_string(),
    })
}

#[tauri::command]
fn cancel_export(state: State<'_, AppState>) {
    state.export_cancel.store(true, Ordering::SeqCst);
}

fn emit_queue(app: &AppHandle, state: &AppState) {
    let _ = app.emit(batch::QUEUE_EVENT, state.batch.snapshot());
}

#[tauri::command]
fn queue_list(state: State<'_, AppState>) -> Vec<JobView> {
    state.batch.snapshot()
}

/// 把视频加入批量队列；目标从当前项目复制，因此必须先在编辑器里配置好目标。
#[tauri::command]
fn queue_add(app: AppHandle, state: State<'_, AppState>, paths: Vec<String>) -> Result<usize, String> {
    let (targets, ffprobe) = {
        let guard = state.session.lock().map_err(|_| "会话锁已损坏")?;
        let session = guard.as_ref().ok_or("请先打开视频或项目并配置目标")?;
        let targets: Vec<Target> = session.project().targets.iter().filter(|t| t.enabled).cloned().collect();
        (targets, session.config().ffmpeg.ffprobe.clone())
    };
    if targets.is_empty() {
        return Err("当前项目没有启用的目标".into());
    }
    let probe = FfprobeBackend { bin: ffprobe };
    let mut sources = Vec::with_capacity(paths.len());
    for p in paths {
        let path = PathBuf::from(&p);
        let info = probe.probe(&path).map_err(|e| format!("{p}：{e}"))?;
        sources.push(SourceInput { path, duration: info.duration });
    }
    let added = state.batch.add(sources, targets)?;
    emit_queue(&app, &*state);
    Ok(added)
}

#[tauri::command]
fn queue_start(app: AppHandle, state: State<'_, AppState>) -> Vec<JobView> {
    let config = app_config(&app);
    state.batch.start(app.clone(), config);
    emit_queue(&app, &*state);
    state.batch.snapshot()
}

#[tauri::command]
fn queue_cancel(app: AppHandle, state: State<'_, AppState>, job_id: String) -> Result<(), String> {
    state.batch.cancel(&job_id)?;
    emit_queue(&app, &*state);
    Ok(())
}

#[tauri::command]
fn queue_resume(app: AppHandle, state: State<'_, AppState>, job_id: String) -> Result<(), String> {
    state.batch.resume(&job_id)?;
    emit_queue(&app, &*state);
    Ok(())
}

#[tauri::command]
fn queue_remove(app: AppHandle, state: State<'_, AppState>, job_id: String) -> Result<(), String> {
    state.batch.remove(&job_id)?;
    emit_queue(&app, &*state);
    Ok(())
}

/// 把某个已完成扫描的队列任务载入编辑器复核。
#[tauri::command]
fn queue_open(app: AppHandle, state: State<'_, AppState>, job_id: String) -> Result<Snapshot, String> {
    if let Some(msg) = state.busy() {
        return Err(msg.to_string());
    }
    state.batch.ready_for_review(&job_id)?;
    let path = state.batch.project_path(&job_id)?;
    let session = Session::open_project(&path, app_config(&app)).map_err(|e| e.to_string())?;
    if let Some(video) = session.project().video.as_ref() {
        allow_video(&app, Path::new(&video.path));
    }
    let mut guard = state.session.lock().map_err(|_| "会话锁已损坏")?;
    *guard = Some(session);
    Ok(snapshot_from(guard.as_ref().expect("刚刚写入"), &*state))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            tool_status,
            snapshot,
            open_video,
            open_project,
            save_project,
            undo,
            redo,
            add_target,
            update_target,
            accept_segment,
            reject_segment,
            delete_segment,
            extend_track,
            set_keyframe,
            add_manual_segment,
            scan_targets,
            cancel_scan,
            export_video,
            cancel_export,
            queue_list,
            queue_add,
            queue_start,
            queue_cancel,
            queue_resume,
            queue_remove,
            queue_open
        ])
        .run(tauri::generate_context!())
        .expect("VeilFrame 启动失败");
}
