//! 编辑核心：对项目做非破坏性修改，支持撤销与重做。
//!
//! 每个操作都在副本上执行并通过校验后才提交，失败时项目保持原样。
//! 对应 `first_design.md` 2.5（负样本反馈）与 1.5（复核工作台）。

use std::collections::HashSet;

use vf_project::{Keyframe, Project, ProjectError, Quad, Segment, SegmentStatus, Source, Target};

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

    /// 新增目标。id 重复时校验失败，项目保持不变。
    pub fn add_target(&mut self, target: Target) -> Result<(), EditError> {
        self.apply(|p| {
            p.targets.push(target);
            Ok(())
        })
    }

    /// 整体替换同 id 的目标（名称、样式、阈值、启用状态等一次提交）。
    pub fn update_target(&mut self, target: Target) -> Result<(), EditError> {
        self.apply(|p| {
            let slot = p
                .targets
                .iter_mut()
                .find(|t| t.id == target.id)
                .ok_or_else(|| EditError::TargetNotFound(target.id.clone()))?;
            *slot = target;
            Ok(())
        })
    }

    /// 在 `t` 处写入关键帧：已存在（±10ms）则更新，否则插入并保持时间有序。
    /// 编辑后的段标记为人工来源、置信度 1.0。
    pub fn set_keyframe(&mut self, segment_id: &str, t: f64, quad: Quad) -> Result<(), EditError> {
        self.apply(|p| {
            let s = Self::segment_mut(p, segment_id)?;
            if let Some(k) = s.keyframes.iter_mut().find(|k| (k.t - t).abs() < 0.01) {
                k.quad = quad;
                k.conf = 1.0;
            } else {
                s.keyframes.push(Keyframe { t, quad, conf: 1.0 });
                s.keyframes.sort_by(|a, b| a.t.total_cmp(&b.t));
            }
            s.source = Source::Manual;
            Ok(())
        })
    }

    /// 用一次全片扫描的结果替换某目标的全部自动段（人工段保留）。可撤销。
    pub fn replace_auto_segments(&mut self, target_id: &str, segments: Vec<Segment>) -> Result<(), EditError> {
        self.replace_auto_segments_in(target_id, f64::NEG_INFINITY, f64::INFINITY, segments)
    }

    /// 用 `[start, end]` 范围内的扫描结果，替换该目标在此范围内的自动段。
    ///
    /// 与范围相交的旧自动段只去掉范围内的部分，范围外的部分保留（关键帧在切口处补齐）。
    /// 人工段和其他目标不受影响。可撤销。
    pub fn replace_auto_segments_in(
        &mut self,
        target_id: &str,
        start: f64,
        end: f64,
        segments: Vec<Segment>,
    ) -> Result<(), EditError> {
        self.apply(|p| {
            if p.target(target_id).is_none() {
                return Err(EditError::TargetNotFound(target_id.to_string()));
            }
            let old = std::mem::take(&mut p.segments);
            let mut kept = Vec::with_capacity(old.len() + segments.len());
            for s in old {
                let affected = s.target == target_id
                    && s.source == Source::Auto
                    && s.t1 > start
                    && s.t0 < end;
                if affected {
                    kept.extend(outside_range(s, start, end));
                } else {
                    kept.push(s);
                }
            }
            kept.extend(segments);
            make_ids_unique(&mut kept);
            p.segments = kept;
            Ok(())
        })
    }
}

/// 去掉 `[start, end]` 之内的部分，返回剩下的左右两段。
fn outside_range(s: Segment, start: f64, end: f64) -> Vec<Segment> {
    let mut out = Vec::new();
    if s.t0 < start {
        if let Some(keyframes) = keys_for_span(&s.keyframes, s.t0, start) {
            out.push(Segment { t1: start, keyframes, ..s.clone() });
        }
    }
    if s.t1 > end {
        if let Some(keyframes) = keys_for_span(&s.keyframes, end, s.t1) {
            out.push(Segment {
                id: format!("{}-after", s.id),
                t0: end,
                keyframes,
                ..s.clone()
            });
        }
    }
    out
}

/// 为 `[lo, hi]` 取关键帧：区间内的原关键帧保留，两端各补一个取最近关键帧的副本。
/// 没有任何关键帧时返回 `None`，避免产生没有位置的段。
fn keys_for_span(keys: &[Keyframe], lo: f64, hi: f64) -> Option<Vec<Keyframe>> {
    const EPS: f64 = 1e-9;
    if keys.is_empty() {
        return None;
    }
    let nearest = |t: f64| {
        keys.iter()
            .min_by(|a, b| (a.t - t).abs().total_cmp(&(b.t - t).abs()))
            .cloned()
            .expect("keys 非空")
    };
    let mut out: Vec<Keyframe> = keys.iter().filter(|k| k.t > lo + EPS && k.t < hi - EPS).cloned().collect();
    out.insert(0, Keyframe { t: lo, ..nearest(lo) });
    out.push(Keyframe { t: hi, ..nearest(hi) });
    Some(out)
}

/// 同一项目内检测段 id 必须唯一。发生冲突时给后出现的段追加 `-2`、`-3`……
fn make_ids_unique(segments: &mut [Segment]) {
    let mut seen: HashSet<String> = HashSet::new();
    for s in segments.iter_mut() {
        if seen.insert(s.id.clone()) {
            continue;
        }
        let fresh = (2..)
            .map(|n| format!("{}-{n}", s.id))
            .find(|candidate| !seen.contains(candidate))
            .expect("无限序列必定能找到未占用的 id");
        seen.insert(fresh.clone());
        s.id = fresh;
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
            pattern: Some("机密".into()),
            match_mode: vf_project::TextMatchMode::Contains,
            fuzzy_edits: 1,
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

    fn extra_target(id: &str) -> Target {
        let mut t = base_project().targets[0].clone();
        t.id = id.into();
        t.name = id.into();
        t
    }

    #[test]
    fn add_and_update_target_are_undoable() {
        let mut e = Editor::new(base_project());
        e.add_target(extra_target("t2")).unwrap();
        assert!(e.project().target("t2").is_some());

        let mut t2 = e.project().target("t2").unwrap().clone();
        t2.name = "新名字".into();
        e.update_target(t2).unwrap();
        assert_eq!(e.project().target("t2").unwrap().name, "新名字");

        e.undo().unwrap();
        assert_eq!(e.project().target("t2").unwrap().name, "t2");
        e.undo().unwrap();
        assert!(e.project().target("t2").is_none());
    }

    #[test]
    fn duplicate_target_id_is_rejected() {
        let mut e = Editor::new(base_project());
        assert!(e.add_target(base_project().targets[0].clone()).is_err());
        assert_eq!(e.project().targets.len(), 1);
        assert!(!e.can_undo());
    }

    #[test]
    fn set_keyframe_updates_or_inserts_in_time_order() {
        let mut e = Editor::new(base_project());
        let quad = [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]];
        e.set_keyframe("s1", 15.0, quad).unwrap();
        e.set_keyframe("s1", 12.0, quad).unwrap();
        let s = e.project().segment("s1").unwrap();
        assert_eq!(s.keyframes.len(), 3);
        assert!((s.keyframes[1].t - 12.0).abs() < 1e-9);
        assert_eq!(s.keyframes[1].quad, quad);
        assert_eq!(s.source, Source::Manual);

        // 同一时刻再写一次是更新而不是插入
        let quad2 = [[5.0, 5.0]; 4];
        e.set_keyframe("s1", 12.0, quad2).unwrap();
        let s = e.project().segment("s1").unwrap();
        assert_eq!(s.keyframes.len(), 3);
        assert_eq!(s.keyframes[1].quad, quad2);
    }

    #[test]
    fn replace_auto_segments_keeps_manual_and_is_undoable() {
        let mut e = Editor::new(base_project());
        e.add_manual_segment("m2", "t1", 30.0, 33.0, [[0.0; 2]; 4]).unwrap();

        let mut seg = e.project().segment("s1").unwrap().clone();
        seg.id = "auto-new".into();
        seg.source = Source::Auto;
        e.replace_auto_segments("t1", vec![seg]).unwrap();

        let ids: Vec<&str> = e.project().segments.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"auto-new"));
        assert!(ids.contains(&"m2"));
        assert!(!ids.contains(&"s1"), "旧的自动段应被替换");

        e.undo().unwrap();
        assert!(e.project().segment("s1").is_some());
        assert!(e.project().segment("auto-new").is_none());
    }

    #[test]
    fn range_replace_keeps_parts_outside_the_range() {
        // 旧段 s1 为 [10, 20]。重扫 [12, 15] 后，s1 只剩 [10, 12]，尾部为 [15, 20]。
        let mut e = Editor::new(base_project());
        let mut fresh = e.project().segment("s1").unwrap().clone();
        fresh.id = "new".into();
        fresh.t0 = 12.0;
        fresh.t1 = 15.0;
        fresh.keyframes = vec![Keyframe { t: 12.0, quad: [[1.0; 2]; 4], conf: 0.8 }];
        e.replace_auto_segments_in("t1", 12.0, 15.0, vec![fresh]).unwrap();

        let p = e.project();
        let head = p.segment("s1").expect("左侧保留");
        assert_eq!((head.t0, head.t1), (10.0, 12.0));
        let tail = p.segment("s1-after").expect("右侧保留");
        assert_eq!((tail.t0, tail.t1), (15.0, 20.0));
        assert!(p.segment("new").is_some());
        assert!(p.validate().is_ok());
    }

    #[test]
    fn range_replace_renames_colliding_ids() {
        // 新扫描的段 id 与保留下来的旧段 id 冲突时，新段改名，旧段不变
        let mut e = Editor::new(base_project());
        let mut fresh = e.project().segment("s1").unwrap().clone();
        fresh.t0 = 18.0;
        fresh.t1 = 19.0;
        fresh.keyframes = vec![Keyframe { t: 18.0, quad: [[0.0; 2]; 4], conf: 0.9 }];
        e.replace_auto_segments_in("t1", 18.0, 19.0, vec![fresh]).unwrap();
        assert!(e.project().segment("s1").is_some());
        assert!(e.project().segment("s1-2").is_some());
        assert!(e.project().validate().is_ok());
    }
}
