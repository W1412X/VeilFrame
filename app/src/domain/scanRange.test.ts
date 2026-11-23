import { describe, expect, it } from 'vitest';
import { parseScanRange } from './scanRange';

describe('parseScanRange', () => {
  it('两项都留空表示整段视频', () => {
    expect(parseScanRange('', '', 60)).toEqual({ start: 0, end: null });
  });

  it('解析起点与终点', () => {
    expect(parseScanRange('5.5', '12', 60)).toEqual({ start: 5.5, end: 12 });
  });

  it('终点超过时长时截断', () => {
    expect(parseScanRange('0', '999', 60)).toEqual({ start: 0, end: 60 });
  });

  it('拒绝负数、非数字、终点不大于起点、起点超出时长', () => {
    expect(typeof parseScanRange('-1', '', 60)).toBe('string');
    expect(typeof parseScanRange('abc', '', 60)).toBe('string');
    expect(typeof parseScanRange('10', '5', 60)).toBe('string');
    expect(typeof parseScanRange('60', '', 60)).toBe('string');
    expect(typeof parseScanRange('0', 'x', 60)).toBe('string');
  });
});
