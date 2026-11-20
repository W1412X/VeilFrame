//! 编辑核心：对项目做非破坏性修改，支持撤销与重做。
//!
//! 每个操作都在副本上执行并通过校验后才提交，失败时项目保持原样。
//! 对应 `first_design.md` 2.5（负样本反馈）与 1.5（复核工作台）。

use vf_project::{Keyframe, Project, ProjectError, Segment, SegmentStatus, Source};

/// 人工否决时，目标阈值每次上调的步长（ADR 未定，先取保守值）。
pub const NEGATIVE_STEP: f64 = 0.01;
/// 阈值上限。
pub const THRESHOLD_MAX: f64 = 0.95;
/// 跟踪传播的默认延长秒数。
pub const TRACK_EXTEND_SECONDS: f64 = 3.0;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EditError {
    #[error("找不到检测段 {0}")]
    SegmentNotFound(String),
    #[error("找不到目标 {0}")]
    TargetNotFound(String),
    #[error("没有可撤销的操作")]
    NothingToUndo,
    #[error("没有可重做的操作")]
    NothingToRedo,
    #[error("操作未通过校验：{0}")]
    Invalid(#[from] ProjectError),
}

/// 快照式历史记录。上限之外的旧记录会被丢弃。
#[derive(Debug, Clone)]
pub struct History<S: Clone> {
    past: Vec<S>,
    future: Vec<S>,
    limit: usize,
}

impl<S: Clone> History<S> {
    pub fn new(limit: usize) -> Self {
        Self { past: Vec::new(), future: Vec::new(), limit }
    }

    /// 在修改前记录当前状态。新操作会清空重做栈。
    pub fn record(&mut self, before: S) {
        self.past.push(before);
        if self.past.len() > self.limit {
            self.past.remove(0);
        }
        self.future.clear();
    }

    pub fn undo(&mut self, current: S) -> Option<S> {
        let prev = self.past.pop()?;
        self.future.push(current);
        Some(prev)
    }

    pub fn redo(&mut self, current: S) -> Option<S> {
        let next = self.future.pop()?;
        self.past.push(current);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
}

/// 编辑器：持有项目与历史记录，是所有修改的唯一入口。
#[derive(Debug, Clone)]
pub struct Editor {
    project: Project,
    history: History<Project>,
}

impl Editor {
    pub const DEFAULT_HISTORY_LIMIT: usize = 200;

    pub fn new(project: Project) -> Self {
        Self { project, history: History::new(Self::DEFAULT_HISTORY_LIMIT) }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn into_project(self) -> Project {
        self.project
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// 在副本上执行修改。校验通过才记录历史并提交，否则不改变原项目。
    fn apply<F>(&mut self, f: F) -> Result<(), EditError>
    where
        F: FnOnce(&mut Project) -> Result<(), EditError>,
    {
        let mut next = self.project.clone();
        f(&mut next)?;
        next.validate()?;
        self.history.record(std::mem::replace(&mut self.project, next));
        Ok(())
    }

    pub fn undo(&mut self) -> Result<(), EditError> {
        let current = self.project.clone();
        let prev = self.history.undo(current).ok_or(EditError::NothingToUndo)?;
        self.project = prev;
        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), EditError> {
        let current = self.project.clone();
        let next = self.history.redo(current).ok_or(EditError::NothingToRedo)?;
        self.project = next;
        Ok(())
    }

    fn segment_mut<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut Segment, EditError> {
        p.segments
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| EditError::SegmentNotFound(id.to_string()))
    }

    /// 接受待确认或低置信段。
    pub fn accept(&mut self, segment_id: &str) -> Result<(), EditError> {
        self.apply(|p| {
            let s = Self::segment_mut(p, segment_id)?;
            s.status = SegmentStatus::Confirmed;
            Ok(())
        })
    }

    /// 否决段。`negative` 为真时记录负样本，并上调该目标阈值（收紧匹配）。
    pub fn reject(&mut self, segment_id: &str, negative: bool) -> Result<(), EditError> {
        self.apply(|p| {
            let target_id = {
                let s = Self::segment_mut(p, segment_id)?;
                s.status = SegmentStatus::Rejected;
                if negative {
                    let samples = s.keyframes.clone();
                    s.negatives.extend(samples);
                }
                s.target.clone()
            };
            if negative {
                let t = p
                    .targets
                    .iter_mut()
                    .find(|t| t.id == target_id)
                    .ok_or(EditError::TargetNotFound(target_id))?;
                t.threshold = (t.threshold + NEGATIVE_STEP).min(THRESHOLD_MAX);
            }
            Ok(())
        })
    }

    /// 从该段结束处向后延长（跟踪传播的简化形式）。
    pub fn extend_track(&mut self, segment_id: &str) -> Result<(), EditError> {
        self.apply(|p| {
            let duration = p.video.as_ref().map(|v| v.duration);
            let s = Self::segment_mut(p, segment_id)?;
            let mut t1 = s.t1 + TRACK_EXTEND_SECONDS;
            if let Some(d) = duration {
                t1 = t1.min(d);
            }
            s.t1 = t1;
            Ok(())
        })
    }

    /// 删除段。
    pub fn delete_segment(&mut self, segment_id: &str) -> Result<(), EditError> {
        self.apply(|p| {
            let before = p.segments.len();
            p.segments.retain(|s| s.id != segment_id);
            if p.segments.len() == before {
                return Err(EditError::SegmentNotFound(segment_id.to_string()));
            }
            Ok(())
        })
    }

    /// 手动补框：在指定时间区间内添加一个已确认的手动段。
    pub fn add_manual_segment(
        &mut self,
        segment_id: &str,
        target_id: &str,
        t0: f64,
        t1: f64,
        quad: vf_project::Quad,
    ) -> Result<(), EditError> {
        self.apply(|p| {
            if p.target(target_id).is_none() {
                return Err(EditError::TargetNotFound(target_id.to_string()));
            }
            p.segments.push(Segment {
                id: segment_id.to_string(),
                target: target_id.to_string(),
                t0,
                t1,
                status: SegmentStatus::Confirmed,
                source: Source::Manual,
                engine: vf_project::Engine::Auto,
                conf: 1.0,
                keyframes: vec![Keyframe { t: t0, quad, conf: 1.0 }],
                negatives: vec![],
            });
            Ok(())
        })
    }

    /// 修改目标阈值（并校验范围）。
    pub fn set_threshold(&mut self, target_id: &str, value: f64) -> Result<(), EditError> {
        self.apply(|p| {
            let t = p
                .targets
                .iter_mut()
                .find(|t| t.id == target_id)
                .ok_or_else(|| EditError::TargetNotFound(target_id.to_string()))?;
            t.threshold = value;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_project::{Engine, Mode, Style, MaskKind, Target, TargetKind, Video};

    fn base_project() -> Project {
        let mut p = Project::new();
        p.video = Some(Video {
            path: "in.mp4".into(),
            content_hash: "blake3-sampled:x".into(),
            duration: 100.0,
            fps: 30.0,
            width: 1920,
            height: 1080,
        });
        p.targets.push(Target {
            id: "t1".into(),
            name: "机密".into(),
            kind: TargetKind::Text,
            color: "#d1495b".into(),
            variants: vec![],
            engine: Engine::Ocr,
            mode: Mode::Standard,
            threshold: 0.6,
            roi: None,
            style: Style { kind: MaskKind::Mosaic, strength: 0.7, padding: 8, feather: true, motion_pad: true },
            enabled: true,
        });
        p.segments.push(Segment {
            id: "s1".into(),
            target: "t1".into(),
            t0: 10.0,
            t1: 20.0,
            status: SegmentStatus::Pending,
            source: Source::Auto,
            engine: Engine::Ocr,
            conf: 0.7,
            keyframes: vec![Keyframe { t: 10.0, quad: [[0.0; 2]; 4], conf: 0.7 }],
            negatives: vec![],
        });
        p
    }

    fn status(e: &Editor, id: &str) -> SegmentStatus {
        e.project().segment(id).unwrap().status
    }

    #[test]
    fn accept_then_undo_restores_status() {
        let mut e = Editor::new(base_project());
        e.accept("s1").unwrap();
        assert_eq!(status(&e, "s1"), SegmentStatus::Confirmed);
        e.undo().unwrap();
        assert_eq!(status(&e, "s1"), SegmentStatus::Pending);
        e.redo().unwrap();
        assert_eq!(status(&e, "s1"), SegmentStatus::Confirmed);
    }

    #[test]
    fn new_edit_clears_redo_stack() {
        let mut e = Editor::new(base_project());
        e.accept("s1").unwrap();
        e.undo().unwrap();
        e.reject("s1", false).unwrap();
        assert!(!e.can_redo());
    }

    #[test]
    fn negative_reject_tightens_threshold_and_records_sample() {
        let mut e = Editor::new(base_project());
        e.reject("s1", true).unwrap();
        let p = e.project();
        assert!((p.target("t1").unwrap().threshold - 0.61).abs() < 1e-9);
        assert_eq!(p.segment("s1").unwrap().negatives.len(), 1);
    }

    #[test]
    fn plain_reject_keeps_threshold() {
        let mut e = Editor::new(base_project());
        e.reject("s1", false).unwrap();
        assert!((e.project().target("t1").unwrap().threshold - 0.6).abs() < 1e-9);
    }

    #[test]
    fn extend_track_is_capped_by_video_duration() {
        let mut e = Editor::new(base_project());
        e.extend_track("s1").unwrap();
        assert!((e.project().segment("s1").unwrap().t1 - 23.0).abs() < 1e-9);

        let mut p = base_project();
        p.segments[0].t1 = 99.0;
        let mut e2 = Editor::new(p);
        e2.extend_track("s1").unwrap();
        assert!((e2.project().segment("s1").unwrap().t1 - 100.0).abs() < 1e-9);
    }

    #[test]
    fn failed_edit_does_not_change_project_or_history() {
        let mut e = Editor::new(base_project());
        let err = e.extend_track("missing").unwrap_err();
        assert!(matches!(err, EditError::SegmentNotFound(_)));
        assert!(!e.can_undo());
    }

    #[test]
    fn invalid_threshold_is_rejected_by_validation() {
        let mut e = Editor::new(base_project());
        assert!(matches!(e.set_threshold("t1", 2.0), Err(EditError::Invalid(_))));
        assert!((e.project().target("t1").unwrap().threshold - 0.6).abs() < 1e-9);
    }

    #[test]
    fn manual_segment_is_confirmed_and_undoable() {
        let mut e = Editor::new(base_project());
        e.add_manual_segment("m1", "t1", 30.0, 33.0, [[1.0, 1.0]; 4]).unwrap();
        assert_eq!(status(&e, "m1"), SegmentStatus::Confirmed);
        e.delete_segment("m1").unwrap();
        assert!(e.project().segment("m1").is_none());
        e.undo().unwrap();
        assert!(e.project().segment("m1").is_some());
    }

    #[test]
    fn history_limit_drops_oldest() {
        let mut h: History<u32> = History::new(2);
        h.record(1);
        h.record(2);
        h.record(3);
        assert_eq!(h.undo(4), Some(3));
        assert_eq!(h.undo(3), Some(2));
        assert_eq!(h.undo(2), None);
    }
}
