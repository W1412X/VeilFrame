// 编辑器：所有修改都经过这里，支持撤销与重做（对应 vf-core::Editor）。
// 纯函数实现，便于测试；界面只负责调用。

import type { Project, Quad, Segment, SegmentStatus, Target } from './types';

/** 与 vf-core 保持一致的常量。 */
export const NEGATIVE_STEP = 0.01;
export const THRESHOLD_MAX = 0.95;
export const TRACK_EXTEND_SECONDS = 3.0;
const HISTORY_LIMIT = 200;

export interface EditorState {
  project: Project;
  past: Project[];
  future: Project[];
}

export function createEditor(project: Project): EditorState {
  return { project, past: [], future: [] };
}

/** 在副本上执行修改，成功后记入历史并清空重做栈。 */
export function edit(state: EditorState, fn: (p: Project) => void): EditorState {
  const next = structuredClone(state.project);
  fn(next);
  const past = [...state.past, state.project];
  if (past.length > HISTORY_LIMIT) past.shift();
  return { project: next, past, future: [] };
}

export function undo(state: EditorState): EditorState {
  if (state.past.length === 0) return state;
  const prev = state.past[state.past.length - 1];
  return { project: prev, past: state.past.slice(0, -1), future: [state.project, ...state.future] };
}

export function redo(state: EditorState): EditorState {
  if (state.future.length === 0) return state;
  const [next, ...rest] = state.future;
  return { project: next, past: [...state.past, state.project], future: rest };
}

function findSegment(p: Project, id: string): Segment {
  const s = p.segments.find((x) => x.id === id);
  if (!s) throw new Error(`找不到检测段 ${id}`);
  return s;
}

function findTarget(p: Project, id: string): Target {
  const t = p.targets.find((x) => x.id === id);
  if (!t) throw new Error(`找不到目标 ${id}`);
  return t;
}

export function setSegmentStatus(state: EditorState, id: string, status: SegmentStatus): EditorState {
  return edit(state, (p) => {
    findSegment(p, id).status = status;
  });
}

/** 否决。`negative` 为真时记录负样本，并上调该目标阈值。 */
export function rejectSegment(state: EditorState, id: string, negative: boolean): EditorState {
  return edit(state, (p) => {
    const s = findSegment(p, id);
    s.status = 'rejected';
    if (negative) {
      s.negatives.push(...s.keyframes.map((k) => ({ ...k })));
      const t = findTarget(p, s.target);
      t.threshold = Math.min(THRESHOLD_MAX, round2(t.threshold + NEGATIVE_STEP));
    }
  });
}

/** 向后延长（跟踪传播的简化形式），不超过视频时长。 */
export function extendTrack(state: EditorState, id: string): EditorState {
  return edit(state, (p) => {
    const s = findSegment(p, id);
    const limit = p.video ? p.video.duration : Number.POSITIVE_INFINITY;
    s.t1 = Math.min(limit, round2(s.t1 + TRACK_EXTEND_SECONDS));
  });
}

/**
 * 在时刻 t 修改段的四边形（关键帧编辑）。
 * 若 t 处已有关键帧则更新它，否则插入一个新关键帧，置信度沿用段的置信度。
 */
export function setKeyframeQuad(state: EditorState, segId: string, t: number, quad: Quad): EditorState {
  return edit(state, (p) => {
    const s = findSegment(p, segId);
    const existing = s.keyframes.find((k) => Math.abs(k.t - t) < 0.01);
    if (existing) {
      existing.quad = quad;
      existing.conf = 1;
    } else {
      s.keyframes.push({ t, quad, conf: 1 });
      s.keyframes.sort((a, b) => a.t - b.t);
    }
    s.source = 'manual';
  });
}

export function addTarget(state: EditorState, target: Target): EditorState {
  return edit(state, (p) => {
    if (p.targets.some((t) => t.id === target.id)) throw new Error(`目标 id 重复：${target.id}`);
    p.targets.push(target);
  });
}

export function deleteSegment(state: EditorState, id: string): EditorState {
  return edit(state, (p) => {
    findSegment(p, id);
    p.segments = p.segments.filter((x) => x.id !== id);
  });
}

export function updateTarget(state: EditorState, id: string, patch: Partial<Target>): EditorState {
  return edit(state, (p) => {
    const t = findTarget(p, id);
    Object.assign(t, patch);
    if (patch.style) t.style = { ...t.style, ...patch.style };
    if (t.threshold < 0 || t.threshold > 1) throw new Error('阈值必须在 0 ~ 1 之间');
  });
}

function round2(v: number): number {
  return Math.round(v * 100) / 100;
}

/** 视频内某段是否在时刻 t 生效（段在区间内且目标启用、段未否决）。 */
export function activeSegments(p: Project, t: number): Segment[] {
  return p.segments.filter((s) => {
    if (s.status === 'rejected') return false;
    if (t < s.t0 || t > s.t1) return false;
    const tg = p.targets.find((x) => x.id === s.target);
    return tg !== undefined && tg.enabled;
  });
}

/** 指定时刻后的下一个待确认段（按开始时间，循环）。 */
export function nextPending(p: Project, t: number): Segment | undefined {
  const list = p.segments
    .filter((s) => s.status === 'pending' || s.status === 'low')
    .sort((a, b) => a.t0 - b.t0);
  if (list.length === 0) return undefined;
  return list.find((s) => s.t0 > t + 0.01) ?? list[0];
}
