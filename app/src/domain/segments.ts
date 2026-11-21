// 只读视图辅助：把项目数据换算成画面叠加与时间轴需要的信息。
// 所有修改都由 Rust 后端完成，这里不复制编辑逻辑。
import type { Project, Quad, Segment } from './types';

/** 时刻 t 生效的段：在区间内、目标启用、未被否决。 */
export function activeSegments(p: Project, t: number): Segment[] {
  return p.segments.filter((s) => {
    if (s.status === 'rejected') return false;
    if (t < s.t0 || t > s.t1) return false;
    const tg = p.targets.find((x) => x.id === s.target);
    return tg !== undefined && tg.enabled;
  });
}

/** 指定时刻之后的下一个待确认段（按开始时间，循环到最前）。 */
export function nextPending(p: Project, t: number): Segment | undefined {
  const list = p.segments
    .filter((s) => s.status === 'pending' || s.status === 'low')
    .sort((a, b) => a.t0 - b.t0);
  if (list.length === 0) return undefined;
  return list.find((s) => s.t0 > t + 0.01) ?? list[0];
}

/** 取时刻 t 最近的关键帧。段没有关键帧时返回 undefined。 */
export function pickKeyframe(s: Segment, t: number): Segment['keyframes'][number] | undefined {
  let best = s.keyframes[0];
  for (const k of s.keyframes) {
    if (Math.abs(k.t - t) < Math.abs(best.t - t)) best = k;
  }
  return best;
}

export function rectToQuad(x: number, y: number, w: number, h: number): Quad {
  return [
    [x, y],
    [x + w, y],
    [x + w, y + h],
    [x, y + h],
  ];
}

/** 四边形的轴对齐包围盒。 */
export function quadBounds(quad: Quad): { x: number; y: number; w: number; h: number } {
  const xs = quad.map((p) => p[0]);
  const ys = quad.map((p) => p[1]);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
}

export function formatTime(t: number): string {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  return `${String(m).padStart(2, '0')}:${s.toFixed(2).padStart(5, '0')}`;
}
