import type { Project } from '../domain/types';

interface Props {
  project: Project;
  selTarget: string | null;
  busy: boolean;
  scanningTargetId: string | null;
  onSelectTarget: (id: string) => void;
  onToggleTarget: (id: string, enabled: boolean) => void;
  onNewTarget: () => void;
  onScanAll: () => void;
}

export function Sidebar({ project, selTarget, busy, scanningTargetId, onSelectTarget, onToggleTarget, onNewTarget, onScanAll }: Props) {
  const enabled = project.targets.filter((t) => t.enabled).length;
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
    </aside>
  );
}
