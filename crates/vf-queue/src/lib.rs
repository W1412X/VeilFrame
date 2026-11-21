//! 批量任务状态机（对应 `first_design.md` 1.7 与 2.10）。
//!
//! 任务在 Queued → Scanning → Review → Exporting → Done 之间流转。
//! 扫描可取消并保留检查点（已扫描到的时间），之后可续扫。
//! 失败会按 `max_attempts` 自动重试。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Scanning,
    Review,
    Exporting,
    Done,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// 开始扫描（从检查点继续）。
    Start,
    /// 扫描进度：已扫描到视频的第 `scanned_until` 秒。
    Progress { scanned_until: f64 },
    /// 扫描完成，进入复核。
    ScanFinished,
    /// 用户取消。
    Cancel,
    /// 失败，附带错误信息。
    Fail(String),
    /// 开始导出。
    ExportStart,
    /// 导出完成。
    ExportFinished,
    /// 取消后恢复为排队，保留检查点。
    Resume,
    /// 失败后重新排队（清零尝试次数）。
    Retry,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum QueueError {
    #[error("状态 {from:?} 不允许事件 {event:?}")]
    InvalidTransition { from: JobState, event: Event },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub state: JobState,
    pub attempts: u32,
    pub max_attempts: u32,
    /// 扫描检查点（秒）。续扫从这里开始。
    pub checkpoint: f64,
    pub last_error: Option<String>,
}

impl Job {
    pub fn new(id: impl Into<String>, max_attempts: u32) -> Self {
        Self {
            id: id.into(),
            state: JobState::Queued,
            attempts: 0,
            max_attempts: max_attempts.max(1),
            checkpoint: 0.0,
            last_error: None,
        }
    }

    /// 应用一个事件。非法转换返回错误，状态保持不变。
    pub fn apply(&mut self, event: Event) -> Result<(), QueueError> {
        use Event::*;
        use JobState::*;
        let next = match (self.state, &event) {
            (Queued, Start) => {
                self.attempts += 1;
                Scanning
            }
            (Scanning, Progress { scanned_until }) => {
                if *scanned_until > self.checkpoint {
                    self.checkpoint = *scanned_until;
                }
                Scanning
            }
            (Scanning, ScanFinished) => Review,
            (Scanning, Cancel) | (Queued, Cancel) | (Review, Cancel) => Cancelled,
            (Scanning, Fail(msg)) | (Exporting, Fail(msg)) => {
                self.last_error = Some(msg.clone());
                if self.attempts < self.max_attempts {
                    Queued
                } else {
                    Failed
                }
            }
            (Review, ExportStart) => {
                self.last_error = None;
                Exporting
            }
            (Exporting, ExportFinished) => Done,
            (Cancelled, Resume) => Queued,
            (Failed, Retry) => {
                self.attempts = 0;
                self.last_error = None;
                Queued
            }
            (from, _) => {
                return Err(QueueError::InvalidTransition { from, event: event.clone() });
            }
        };
        self.state = next;
        Ok(())
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.state, JobState::Done | JobState::Failed)
    }
}

/// FIFO 队列，只负责挑选下一个可执行的任务。
#[derive(Debug, Default)]
pub struct JobQueue {
    jobs: Vec<Job>,
}

impl JobQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, job: Job) {
        self.jobs.push(job);
    }

    pub fn get(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// 第一个处于 Queued 状态的任务。
    pub fn next_queued(&self) -> Option<&Job> {
        self.jobs.iter().find(|j| j.state == JobState::Queued)
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_runs_to_done() {
        let mut j = Job::new("a", 3);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Progress { scanned_until: 30.0 }).unwrap();
        j.apply(Event::ScanFinished).unwrap();
        j.apply(Event::ExportStart).unwrap();
        j.apply(Event::ExportFinished).unwrap();
        assert_eq!(j.state, JobState::Done);
        assert!(j.is_terminal());
        assert_eq!(j.checkpoint, 30.0);
    }

    #[test]
    fn cancel_keeps_checkpoint_and_resume_requeues() {
        let mut j = Job::new("a", 3);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Progress { scanned_until: 42.0 }).unwrap();
        j.apply(Event::Cancel).unwrap();
        assert_eq!(j.state, JobState::Cancelled);
        j.apply(Event::Resume).unwrap();
        assert_eq!(j.state, JobState::Queued);
        assert_eq!(j.checkpoint, 42.0);
    }

    #[test]
    fn checkpoint_never_moves_backwards() {
        let mut j = Job::new("a", 3);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Progress { scanned_until: 50.0 }).unwrap();
        j.apply(Event::Progress { scanned_until: 20.0 }).unwrap();
        assert_eq!(j.checkpoint, 50.0);
    }

    #[test]
    fn failure_retries_until_max_attempts() {
        let mut j = Job::new("a", 2);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Fail("ffmpeg 崩溃".into())).unwrap();
        assert_eq!(j.state, JobState::Queued);
        assert_eq!(j.last_error.as_deref(), Some("ffmpeg 崩溃"));

        j.apply(Event::Start).unwrap();
        j.apply(Event::Fail("again".into())).unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert!(j.is_terminal());
    }

    #[test]
    fn failed_job_can_be_retried_manually() {
        let mut j = Job::new("a", 1);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Fail("bad".into())).unwrap();
        assert_eq!(j.state, JobState::Failed);
        j.apply(Event::Retry).unwrap();
        assert_eq!(j.state, JobState::Queued);
        assert_eq!(j.attempts, 0);
        assert!(j.last_error.is_none());
    }

    #[test]
    fn invalid_transition_is_rejected_and_state_unchanged() {
        let mut j = Job::new("a", 3);
        let err = j.apply(Event::ScanFinished).unwrap_err();
        assert!(matches!(err, QueueError::InvalidTransition { from: JobState::Queued, .. }));
        assert_eq!(j.state, JobState::Queued);
    }

    #[test]
    fn queue_picks_first_queued_job_in_order() {
        let mut q = JobQueue::new();
        q.push(Job::new("a", 3));
        q.push(Job::new("b", 3));
        q.get_mut("a").unwrap().apply(Event::Start).unwrap();
        assert_eq!(q.next_queued().map(|j| j.id.as_str()), Some("b"));
    }
}
