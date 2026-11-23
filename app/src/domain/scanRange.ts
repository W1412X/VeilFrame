// 扫描范围的输入解析。输入框的文本在这里校验，失败时返回中文错误信息。

export interface ScanRange {
  /** 起点（秒）。 */
  start: number;
  /** 终点（秒）；null 表示扫到视频末尾。 */
  end: number | null;
}

/** 目标自己的扫描范围（秒），终点已截断到视频时长。与后端 `target_range` 一致。 */
export function targetScanRange(
  target: { scan_start: number; scan_end: number | null },
  duration: number,
): { start: number; end: number } {
  const start = Math.min(Math.max(target.scan_start, 0), duration);
  const end = Math.min(target.scan_end ?? duration, duration);
  return { start, end };
}

/**
 * 解析起点与终点输入。起点留空视为 0，终点留空视为视频末尾。
 * 终点超过视频时长时截断到视频末尾。
 */
export function parseScanRange(startText: string, endText: string, duration: number): ScanRange | string {
  const start = startText.trim() === '' ? 0 : Number(startText);
  if (!Number.isFinite(start) || start < 0) return '扫描起点必须是不小于 0 的数字';

  let end: number | null = null;
  if (endText.trim() !== '') {
    end = Number(endText);
    if (!Number.isFinite(end)) return '扫描终点必须是数字';
  }

  if (start >= duration) return '扫描起点超出视频时长';
  const effectiveEnd = end ?? duration;
  if (effectiveEnd <= start) return '扫描终点必须大于起点';
  return { start, end: end === null ? null : Math.min(end, duration) };
}
