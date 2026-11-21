//! 批量队列：每个视频一个独立会话，按队列顺序扫描并把项目保存到视频同目录。
//!
//! 与主编辑器共用同一套 `vf-session` 扫描路径，但互不占用互斥锁；队列任务
//! 扫描完成后停在「待复核」，由用户从队列切回编辑器逐条检查、导出。
//! 状态机复用 `vf-queue`，失败自动重试，取消保留已扫结果。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use vf_project::{SegmentStatus, Target};
use vf_queue::{Event, Job, JobState};
use vf_session::{ScanOptions, Session, SessionConfig};

pub const QUEUE_EVENT: &str = "vf-queue-changed";

/// 前端展示的一条队列记录。
#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: String,
    pub file: String,
    pub source: String,
    pub project_path: String,
    pub state: &'static str,
    pub progress: f64,
    pub duration: f64,
    pub found: usize,
    pub pending: usize,
    pub attempts: u32,
    pub error: Option<String>,
}

struct Record {
    job: Job,
    source: PathBuf,
    project_path: PathBuf,
    targets: Vec<Target>,
    duration: f64,
    progress: f64,
    found: usize,
    pending: usize,
}

pub struct SourceInput {
    pub path: PathBuf,
    pub duration: f64,
}

struct Inner {
    jobs: Vec<Record>,
    next_id: u64,
    current: Option<String>,
    last_emit: Option<Instant>,
}

impl Default for Inner {
    fn default() -> Self {
        Self { jobs: Vec::new(), next_id: 0, current: None, last_emit: None }
    }
}

pub struct BatchManager {
    inner: Mutex<Inner>,
    running: AtomicBool,
    cancel: AtomicBool,
}

impl Default for BatchManager {
    fn default() -> Self {
        Self::new()
    }
}

fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Scanning => "scanning",
        JobState::Review => "review",
        JobState::Exporting => "exporting",
        JobState::Done => "done",
        JobState::Cancelled => "cancelled",
        JobState::Failed => "failed",
    }
}

impl BatchManager {
    pub fn new() -> Self {
        Self { inner: Mutex::new(Inner::default()), running: AtomicBool::new(false), cancel: AtomicBool::new(false) }
    }

    pub fn snapshot(&self) -> Vec<JobView> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .jobs
            .iter()
            .map(|r| {
                let review = r.job.state == JobState::Review || r.job.state == JobState::Done;
                JobView {
                    id: r.job.id.clone(),
                    file: r.source.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    source: r.source.display().to_string(),
                    project_path: r.project_path.display().to_string(),
                    state: state_name(r.job.state),
                    progress: if review { r.duration } else { r.progress },
                    duration: r.duration,
                    found: r.found,
                    pending: r.pending,
                    attempts: r.job.attempts,
                    error: r.job.last_error.clone(),
                }
            })
            .collect()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 添加视频；已在本队列中的源文件跳过。返回新增数量。
    pub fn add(&self, sources: Vec<SourceInput>, targets: Vec<Target>) -> Result<usize, String> {
        if sources.is_empty() {
            return Err("没有选择视频".into());
        }
        if targets.is_empty() {
            return Err("当前项目没有启用的目标".into());
        }
        let mut inner = self.inner.lock().map_err(|_| "队列锁已损坏")?;
        let mut known: HashSet<PathBuf> = inner.jobs.iter().map(|r| r.source.clone()).collect();
        let mut added = 0;
        for src in sources {
            if !known.insert(src.path.clone()) {
                continue;
            }
            inner.next_id += 1;
            let id = format!("j{}", inner.next_id);
            let project_path = unique_project_path(&src.path);
            inner.jobs.push(Record {
                job: Job::new(id, 2),
                source: src.path,
                project_path,
                targets: targets.clone(),
                duration: src.duration,
                progress: 0.0,
                found: 0,
                pending: 0,
            });
            added += 1;
        }
        Ok(added)
    }

    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "队列锁已损坏")?;
        let is_current = inner.current.as_deref() == Some(id);
        let rec = inner.jobs.iter_mut().find(|r| r.job.id == id).ok_or("任务不存在")?;
        match rec.job.state {
            JobState::Queued => rec.job.apply(Event::Cancel).map_err(|e| e.to_string()),
            JobState::Scanning if is_current => {
                self.cancel.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err("该任务当前不能取消".into()),
        }
    }

    pub fn resume(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "队列锁已损坏")?;
        let rec = inner.jobs.iter_mut().find(|r| r.job.id == id).ok_or("任务不存在")?;
        let event = match rec.job.state {
            JobState::Cancelled => Event::Resume,
            JobState::Failed => Event::Retry,
            _ => return Err("该任务不需要重新排队".into()),
        };
        rec.job.apply(event).map_err(|e| e.to_string())
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "队列锁已损坏")?;
        if inner.current.as_deref() == Some(id) {
            return Err("任务正在扫描，先停止再移除".into());
        }
        let before = inner.jobs.len();
        inner.jobs.retain(|r| r.job.id != id);
        if inner.jobs.len() == before {
            return Err("任务不存在".into());
        }
        Ok(())
    }

    pub fn project_path(&self, id: &str) -> Result<PathBuf, String> {
        let inner = self.inner.lock().map_err(|_| "队列锁已损坏")?;
        inner.jobs.iter().find(|r| r.job.id == id).map(|r| r.project_path.clone()).ok_or_else(|| "任务不存在".into())
    }

    /// 复核后在编辑器里导出成功，把任务标记为完成。不存在或状态不符时静默忽略。
    pub fn mark_done(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rec) = inner.jobs.iter_mut().find(|r| r.job.id == id) else { return };
        if rec.job.state == JobState::Review {
            let _ = rec.job.apply(Event::ExportStart);
        }
        if rec.job.state == JobState::Exporting {
            let _ = rec.job.apply(Event::ExportFinished);
        }
    }

    /// 扫描完成后才允许切回编辑器复核。
    pub fn ready_for_review(&self, id: &str) -> Result<(), String> {
        let inner = self.inner.lock().map_err(|_| "队列锁已损坏")?;
        let rec = inner.jobs.iter().find(|r| r.job.id == id).ok_or("任务不存在")?;
        if matches!(rec.job.state, JobState::Review | JobState::Done) {
            Ok(())
        } else {
            Err("该任务还没有扫描完成".into())
        }
    }

    /// 启动后台工作线程；已在运行时直接返回。
    pub fn start(self: &Arc<Self>, app: AppHandle, config: SessionConfig) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = Arc::clone(self);
        std::thread::spawn(move || manager.worker(app, config));
    }

    fn worker(&self, app: AppHandle, config: SessionConfig) {
        loop {
            let Some(run) = self.take_next() else {
                // 收尾：先标记停止，再复查一次，避免与开始/结束瞬间的入队竞争。
                self.running.store(false, Ordering::SeqCst);
                if self.has_queued() && !self.running.swap(true, Ordering::SeqCst) {
                    continue;
                }
                break;
            };
            self.emit(&app);
            self.cancel.store(false, Ordering::SeqCst);

            match self.run_job(&run, &config, &app) {
                Ok(stats) => {
                    let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    inner.current = None;
                    if let Some(rec) = inner.jobs.iter_mut().find(|r| r.job.id == run.id) {
                        rec.found = stats.found;
                        rec.pending = stats.pending;
                        let _ = rec.job.apply(Event::ScanFinished);
                    }
                }
                Err(JobFailure::Cancelled) => self.finish_cancelled(&run.id),
                Err(JobFailure::Error(msg)) => {
                    // 未达重试上限时状态回到 Queued，下一轮会自动再取到它。
                    self.fail(&run.id, &msg);
                }
            }
            self.emit(&app);
        }
        self.emit(&app);
    }

    fn has_queued(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.jobs.iter().any(|r| r.job.state == JobState::Queued)
    }

    fn run_job(&self, run: &Run, config: &SessionConfig, app: &AppHandle) -> Result<Stats, JobFailure> {
        let mut session = Session::open_video(&run.source, config.clone())
            .map_err(|e| JobFailure::Error(format!("打开视频失败：{e}")))?;
        for target in &run.targets {
            session.add_target(target.clone()).map_err(|e| JobFailure::Error(format!("写入目标失败：{e}")))?;
        }
        session
            .save(Some(&run.project_path))
            .map_err(|e| JobFailure::Error(format!("创建项目失败：{e}")))?;
        let ids: Vec<String> = run.targets.iter().map(|t| t.id.clone()).collect();
        let summary = session
            .scan_targets(&ids, ScanOptions::default(), &self.cancel, |p| {
                self.progress(&run.id, run.duration, p.target_index, p.target_total, p.t, p.duration);
                self.emit(app);
            })
            .map_err(|e| JobFailure::Error(e.to_string()))?;
        if summary.cancelled {
            return Err(JobFailure::Cancelled);
        }
        let project = session.project();
        let found = project.segments.iter().filter(|s| s.status != SegmentStatus::Rejected).count();
        let pending = project
            .segments
            .iter()
            .filter(|s| matches!(s.status, SegmentStatus::Pending | SegmentStatus::Low))
            .count();
        Ok(Stats { found, pending })
    }

    fn take_next(&self) -> Option<Run> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let id = inner.jobs.iter().find(|r| r.job.state == JobState::Queued)?.job.id.clone();
        let rec = inner.jobs.iter_mut().find(|r| r.job.id == id)?;
        rec.job.apply(Event::Start).ok()?;
        rec.found = 0;
        rec.pending = 0;
        rec.progress = 0.0;
        let run = Run {
            id: id.clone(),
            source: rec.source.clone(),
            project_path: rec.project_path.clone(),
            targets: rec.targets.clone(),
            duration: rec.duration,
        };
        inner.current = Some(id);
        Some(run)
    }

    fn progress(&self, id: &str, duration: f64, index: usize, total: usize, t: f64, target_duration: f64) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rec) = inner.jobs.iter_mut().find(|r| r.job.id == id) else { return };
        // 一条任务含多个目标时，用目标序号把进度铺到整条任务上。
        let fraction = ((index as f64 + t / target_duration.max(1e-6)) / total.max(1) as f64).clamp(0.0, 1.0);
        rec.progress = fraction * duration;
    }

    fn finish_cancelled(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.current = None;
        if let Some(rec) = inner.jobs.iter_mut().find(|r| r.job.id == id) {
            let _ = rec.job.apply(Event::Cancel);
        }
    }

    /// 失败：未达上限时状态回到 Queued，下一轮会自动再取到它。
    fn fail(&self, id: &str, msg: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.current = None;
        if let Some(rec) = inner.jobs.iter_mut().find(|r| r.job.id == id) {
            let _ = rec.job.apply(Event::Fail(msg.to_string()));
        }
    }

    fn emit(&self, app: &AppHandle) {
        let due = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let due = inner.last_emit.map_or(true, |last| now.duration_since(last) >= Duration::from_millis(250));
            if due {
                inner.last_emit = Some(now);
            }
            due
        };
        if due {
            let _ = app.emit(QUEUE_EVENT, self.snapshot());
        }
    }
}

struct Run {
    id: String,
    source: PathBuf,
    project_path: PathBuf,
    targets: Vec<Target>,
    duration: f64,
}

struct Stats {
    found: usize,
    pending: usize,
}

enum JobFailure {
    Cancelled,
    Error(String),
}

/// 项目写在视频同目录；同名项目已存在时加序号，避免覆盖用户文件。
fn unique_project_path(source: &Path) -> PathBuf {
    let dir = source.parent().unwrap_or_else(|| Path::new("."));
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let first = dir.join(format!("{stem}.maskproj"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let p = dir.join(format!("{stem}-{n}.maskproj"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem}-{}.maskproj", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_project_path_avoids_overwrite() {
        let dir = std::env::temp_dir().join(format!("vf-batch-{}-unique", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("clip.mp4");
        assert_eq!(unique_project_path(&video), dir.join("clip.maskproj"));
        std::fs::write(dir.join("clip.maskproj"), b"x").unwrap();
        assert_eq!(unique_project_path(&video), dir.join("clip-2.maskproj"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
