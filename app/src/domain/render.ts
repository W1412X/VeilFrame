// 打码参数与几何计算，与 Rust 端 vf-export（quad_at）和 vf-render 的公式保持一致。
// 预览与导出共用这些规则，保证两边看到的打码区域和强度相同。
import type { Keyframe, Quad, Segment } from './types';

export interface KeyframeSample {
  quad: Quad;
  /** 运动速度，单位为像素/帧，用于运动自适应外扩。 */
  speed: number;
}

const clamp01 = (v: number) => Math.min(1, Math.max(0, v));

/** 马赛克块边长（像素），4 到 24。 */
export function mosaicBlock(strength: number): number {
  return 4 + Math.round(clamp01(strength) * 20);
}

/** 高斯模糊的 sigma，1 到 9。 */
export function blurSigma(strength: number): number {
  return 1 + clamp01(strength) * 8;
}

/** 运动自适应外扩：基础外扩加上 2 倍速度，最多再加 16 像素。 */
export function motionPadding(basePx: number, speed: number): number {
  return basePx + Math.min(speed * 2, 16);
}

/** 从质心沿径向外推 `pad` 像素。 */
export function expandQuad(q: Quad, pad: number): Quad {
  const cx = (q[0][0] + q[1][0] + q[2][0] + q[3][0]) / 4;
  const cy = (q[0][1] + q[1][1] + q[2][1] + q[3][1]) / 4;
  return q.map(([x, y]) => {
    const vx = x - cx;
    const vy = y - cy;
    const len = Math.hypot(vx, vy);
    if (len < 1e-9) return [x, y];
    return [x + (vx / len) * pad, y + (vy / len) * pad];
  }) as Quad;
}

/**
 * 时刻 t 处的四边形：相邻关键帧之间线性插值，超出范围时取最近的关键帧。
 * 同时返回该段的运动速度（像素/帧），与导出时的计算相同。
 */
export function quadAt(seg: Segment, t: number, fps: number): KeyframeSample {
  const ks: Keyframe[] = seg.keyframes;
  if (ks.length === 0) return { quad: [[0, 0], [0, 0], [0, 0], [0, 0]], speed: 0 };
  const first = ks[0];
  const last = ks[ks.length - 1];
  if (t <= first.t || ks.length === 1) return { quad: first.quad, speed: 0 };
  if (t >= last.t) return { quad: last.quad, speed: 0 };

  const found = ks.findIndex((k) => k.t > t);
  const i = Math.max(found === -1 ? ks.length - 1 : found, 1);
  const a = ks[i - 1];
  const b = ks[i];
  const f = b.t > a.t ? (t - a.t) / (b.t - a.t) : 0;

  const quad = a.quad.map((p, c) => [
    p[0] + (b.quad[c][0] - p[0]) * f,
    p[1] + (b.quad[c][1] - p[1]) * f,
  ]) as Quad;

  const center = (q: Quad) => [(q[0][0] + q[2][0]) / 2, (q[0][1] + q[2][1]) / 2];
  const ca = center(a.quad);
  const cb = center(b.quad);
  const dist = Math.hypot(cb[0] - ca[0], cb[1] - ca[1]);
  const framesBetween = (b.t - a.t) * fps;
  const speed = framesBetween > 0 ? dist / framesBetween : 0;
  return { quad, speed };
}
