import { describe, expect, it } from 'vitest';
import type { Quad, Segment } from './types';
import { blurSigma, expandQuad, mosaicBlock, motionPadding, quadAt } from './render';

function segment(keys: { t: number; quad: Quad }[]): Segment {
  return {
    id: 's',
    target: 't',
    t0: keys[0].t,
    t1: keys[keys.length - 1].t,
    status: 'confirmed',
    source: 'auto',
    engine: 'ocr',
    conf: 0.9,
    keyframes: keys.map((k) => ({ ...k, conf: 0.9 })),
    negatives: [],
  };
}

const boxAt = (x: number): Quad => [
  [x, 0],
  [x + 10, 0],
  [x + 10, 10],
  [x, 10],
];

describe('打码强度映射', () => {
  it('马赛克块边长与 Rust 端一致', () => {
    expect(mosaicBlock(0)).toBe(4);
    expect(mosaicBlock(0.5)).toBe(14);
    expect(mosaicBlock(1)).toBe(24);
    expect(mosaicBlock(2)).toBe(24);
  });

  it('模糊 sigma 在 1 到 9 之间', () => {
    expect(blurSigma(0)).toBe(1);
    expect(blurSigma(1)).toBe(9);
  });

  it('运动外扩最多增加 16 像素', () => {
    expect(motionPadding(4, 0)).toBe(4);
    expect(motionPadding(4, 100)).toBe(20);
  });
});

describe('expandQuad', () => {
  it('沿径向向外扩展', () => {
    const e = expandQuad(boxAt(10), 2);
    expect(e[0][0]).toBeLessThan(10);
    expect(e[2][0]).toBeGreaterThan(20);
    expect(e[0][1]).toBeLessThan(0);
  });
});

describe('quadAt', () => {
  it('在相邻关键帧之间线性插值，并给出速度', () => {
    const seg = segment([
      { t: 0, quad: boxAt(0) },
      { t: 2, quad: boxAt(20) },
    ]);
    const { quad, speed } = quadAt(seg, 1, 30);
    expect(quad[0][0]).toBeCloseTo(10);
    // 中心从 x=5 移到 x=25，共 20 px；2 秒 × 30 帧 = 60 帧，速度 1/3 px/帧
    expect(speed).toBeCloseTo(20 / 60);
  });

  it('超出关键帧范围时取边界值，速度为 0', () => {
    const seg = segment([
      { t: 1, quad: boxAt(0) },
      { t: 3, quad: boxAt(20) },
    ]);
    expect(quadAt(seg, 0, 30)).toEqual({ quad: boxAt(0), speed: 0 });
    expect(quadAt(seg, 9, 30)).toEqual({ quad: boxAt(20), speed: 0 });
  });

  it('只有一个关键帧时保持不变', () => {
    const seg = segment([{ t: 1, quad: boxAt(5) }]);
    expect(quadAt(seg, 1.5, 30).quad).toEqual(boxAt(5));
  });
});
