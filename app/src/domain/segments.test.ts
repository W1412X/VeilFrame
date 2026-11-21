import { describe, expect, it } from 'vitest';
import type { Project, Quad, Segment, Target } from './types';
import { activeSegments, nextPending, pickKeyframe, quadBounds, rectToQuad } from './segments';

const quad: Quad = [
  [10, 20],
  [110, 20],
  [110, 70],
  [10, 70],
];

function target(id: string, enabled = true): Target {
  return {
    id,
    name: id,
    kind: 'text',
    color: '#d1495b',
    variants: [],
    pattern: 'x',
    match_mode: 'contains',
    fuzzy_edits: 1,
    engine: 'ocr',
    mode: 'standard',
    threshold: 0.6,
    roi: null,
    style: { kind: 'mosaic', strength: 0.6, padding: 4, feather: true, motion_pad: false },
    enabled,
  };
}

function segment(id: string, over: Partial<Segment> = {}): Segment {
  return {
    id,
    target: 't1',
    t0: 1,
    t1: 2,
    status: 'pending',
    source: 'auto',
    engine: 'ocr',
    conf: 0.8,
    keyframes: [{ t: 1, quad, conf: 0.8 }],
    negatives: [],
    ...over,
  };
}

function project(segments: Segment[], targets = [target('t1')]): Project {
  return {
    format_version: 1,
    video: null,
    targets,
    segments,
    render: { codec: 'h264', smart_render: false },
  };
}

describe('activeSegments', () => {
  it('只保留区间内且目标启用的段', () => {
    const p = project([segment('a'), segment('b', { t0: 5, t1: 6 })]);
    expect(activeSegments(p, 1.5).map((s) => s.id)).toEqual(['a']);
    expect(activeSegments(p, 0.5)).toEqual([]);
  });

  it('排除已否决段与停用目标', () => {
    const p = project([segment('a'), segment('b', { status: 'rejected' })], [target('t1', false)]);
    expect(activeSegments(p, 1.5)).toEqual([]);
  });
});

describe('nextPending', () => {
  it('取当前之后最早的待确认段，末尾循环', () => {
    const p = project([
      segment('a', { t0: 1, t1: 2 }),
      segment('b', { t0: 5, t1: 6, status: 'low' }),
      segment('c', { t0: 3, t1: 4, status: 'confirmed' }),
    ]);
    expect(nextPending(p, 0)?.id).toBe('a');
    expect(nextPending(p, 1.5)?.id).toBe('b');
    expect(nextPending(p, 9)?.id).toBe('a');
  });

  it('没有待确认段时返回 undefined', () => {
    expect(nextPending(project([segment('a', { status: 'confirmed' })]), 0)).toBeUndefined();
  });
});

describe('pickKeyframe', () => {
  it('选择时间上最近的关键帧', () => {
    const s = segment('a', {
      keyframes: [
        { t: 0, quad, conf: 1 },
        { t: 10, quad: quad.map(([x, y]) => [x + 5, y]) as Quad, conf: 1 },
      ],
    });
    expect(pickKeyframe(s, 4)?.t).toBe(0);
    expect(pickKeyframe(s, 7)?.t).toBe(10);
  });
});

describe('几何辅助', () => {
  it('rectToQuad 与 quadBounds 互逆', () => {
    const q = rectToQuad(10, 20, 100, 50);
    expect(q).toEqual(quad);
    expect(quadBounds(q)).toEqual({ x: 10, y: 20, w: 100, h: 50 });
  });
});
