import type { Project } from '../domain/types';

interface Props {
  project: Project;
  selTarget: string | null;
  onSelectTarget: (id: string) => void;
  onToggleTarget: (id: string, enabled: boolean) => void;
  onNewTarget: () => void;
}

export function Sidebar({ project, selTarget, onSelectTarget, onToggleTarget, onNewTarget }: Props) {
  return (
    <aside className="sidebar">
      <div className="sec">目标</div>
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
          <div key={t.id} className={`tg ${selTarget === t.id ? 'on' : ''} ${t.enabled ? '' : 'off'}`} onClick={() => onSelectTarget(t.id)}>
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
          </div>
        );
      })}
      <button className="sm" style={{ marginTop: 12 }} onClick={onNewTarget}>
        + 新建目标
      </button>
    </aside>
  );
}
