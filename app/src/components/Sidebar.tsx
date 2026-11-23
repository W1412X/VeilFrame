import type { Project } from '../domain/types';

/** 线程数允许的范围，与后端 MAX_SCAN_THREADS 一致。 */
export const MIN_THREADS = 1;
export const MAX_THREADS = 16;

interface Props {
  project: Project;
  selTarget: string | null;
  busy: boolean;
  scanningTargetId: string | null;
  onSelectTarget: (id: string) => void;
  onToggleTarget: (id: string, enabled: boolean) => void;
  onNewTarget: () => void;
  onScanAll: () => void;
  /** 扫描范围的输入文本（秒），留空分别表示 0 与视频末尾。 */
  scanStart: string;
  scanEnd: string;
  onScanStartChange: (value: string) => void;
  onScanEndChange: (value: string) => void;
  threads: number;
  onThreadsChange: (value: number) => void;
}

export function Sidebar({
  project,
  selTarget,
  busy,
  scanningTargetId,
  onSelectTarget,
  onToggleTarget,
  onNewTarget,
  onScanAll,
  scanStart,
  scanEnd,
  onScanStartChange,
  onScanEndChange,
  threads,
  onThreadsChange,
}: Props) {
  const enabled = project.targets.filter((t) => t.enabled).length;
  const duration = project.video?.duration ?? 0;
  return (
    <aside className="sidebar">
      <div className="sec-row">
        <div className="sec">目标</div>
        <button className="sm" disabled={busy || enabled === 0} title="扫描所有启用的目标" onClick={onScanAll}>
          扫描全部
        </button>
      </div>
      {project.targets.length === 0 && <p className="muted small">还没有目标。新建文本、图像或固定区域目标后开始扫描。</p>}
      {project.targets.map((t) => {
        const count = project.segments.filter((s) => s.target === t.id && s.status !== 'rejected').length;
        const pending = project.segments.filter(
          (s) => s.target === t.id && (s.status === 'pending' || s.status === 'low'),
        ).length;
        let sub: string;
        if (!t.enabled) sub = '未启用';
        else if (count === 0) sub = '未检出';
        else sub = `${count} 段${pending ? ` · ${pending} 待确认` : ''}`;
        return (
          <div
            key={t.id}
            className={`tg ${selTarget === t.id ? 'on' : ''} ${t.enabled ? '' : 'off'}`}
            onClick={() => onSelectTarget(t.id)}
          >
            <input
              type="checkbox"
              checked={t.enabled}
              onClick={(e) => e.stopPropagation()}
              onChange={(e) => onToggleTarget(t.id, e.target.checked)}
            />
            <span className="dot" style={{ background: t.color }} />
            <div className="name">
              {t.name}
              <small>
                {t.kind === 'text' ? '文本' : t.kind === 'image' ? '图像' : '区域'} · {sub}
              </small>
            </div>
            {scanningTargetId === t.id && <span className="badge scanning">扫描中</span>}
          </div>
        );
      })}
      <button className="sm" style={{ marginTop: 12 }} disabled={busy} onClick={onNewTarget}>
        + 新建目标
      </button>

      <div className="sec" style={{ marginTop: 22 }}>
        扫描设置
      </div>
      <div className="scan-settings">
        <label>
          起点（秒）
          <input
            type="number"
            min={0}
            step={0.5}
            value={scanStart}
            disabled={busy}
            onChange={(e) => onScanStartChange(e.target.value)}
          />
        </label>
        <label>
          终点（秒）
          <input
            type="number"
            min={0}
            step={0.5}
            value={scanEnd}
            placeholder={duration > 0 ? duration.toFixed(1) : '末尾'}
            disabled={busy}
            onChange={(e) => onScanEndChange(e.target.value)}
          />
        </label>
        <p className="muted small">终点留空表示扫到视频末尾。</p>
        <label>
          检测线程数
          <input
            type="number"
            min={MIN_THREADS}
            max={MAX_THREADS}
            step={1}
            value={threads}
            disabled={busy}
            onChange={(e) => {
              const n = Math.round(Number(e.target.value));
              if (Number.isFinite(n)) onThreadsChange(Math.min(MAX_THREADS, Math.max(MIN_THREADS, n)));
            }}
          />
        </label>
        <p className="muted small">默认 4。线程越多占用 CPU 越多。</p>
      </div>
    </aside>
  );
}
