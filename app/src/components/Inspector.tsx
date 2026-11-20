import type { MaskKind, Mode, Project, Target } from '../domain/types';
import { activeSegments } from '../domain/editor';

interface Props {
  project: Project;
  selTarget: string | null;
  selSeg: string | null;
  negativeSample: boolean;
  onNegativeSample: (v: boolean) => void;
  onAccept: (id: string) => void;
  onReject: (id: string) => void;
  onTrack: (id: string) => void;
  onDelete: (id: string) => void;
  onUpdateTarget: (id: string, patch: Partial<Target>) => void;
  onUpdateStyle: (id: string, patch: Partial<Target['style']>) => void;
  time: number;
}

const MODES: { value: Mode; label: string }[] = [
  { value: 'fast', label: '极速' },
  { value: 'standard', label: '标准' },
  { value: 'precise', label: '精准' },
];

const KINDS: { value: MaskKind; label: string }[] = [
  { value: 'mosaic', label: '马赛克' },
  { value: 'blur', label: '模糊' },
  { value: 'solid', label: '色块' },
];

const STATUS_LABEL = { confirmed: '已确认', pending: '待确认', low: '低置信', rejected: '已否决' };

export function Inspector(props: Props) {
  const { project, selTarget, selSeg, negativeSample, onNegativeSample, onAccept, onReject, onTrack, onDelete, onUpdateTarget, onUpdateStyle, time } = props;
  const t = project.targets.find((x) => x.id === selTarget);
  const s = project.segments.find((x) => x.id === selSeg);
  if (!t) return <aside className="inspector"><p className="muted">选择左侧目标，或画面中的检测段。</p></aside>;

  const visible = activeSegments(project, time).filter((x) => x.target === t.id).length;
  const totalForTarget = project.segments.filter((x) => x.target === t.id && x.status !== 'rejected').length;

  return (
    <aside className="inspector">
      {s && s.target === t.id && (
        <section>
          <h4>检测段</h4>
          <div className="kv"><span>时间</span><b>{s.t0.toFixed(1)} – {s.t1.toFixed(1)} s</b></div>
          <div className="kv"><span>状态</span><span className={`badge ${s.status}`}>{STATUS_LABEL[s.status]}</span></div>
          <div className="kv"><span>置信度</span><b>{s.conf.toFixed(2)}</b></div>
          <div className="kv"><span>引擎</span><b>{s.engine}</b></div>
          <div className="btns">
            <button className="primary" disabled={s.status === 'confirmed'} onClick={() => onAccept(s.id)}>接受</button>
            <button disabled={s.status === 'rejected'} onClick={() => onReject(s.id)}>否决</button>
            <button className="ghost" onClick={() => onDelete(s.id)}>删除</button>
          </div>
          <label className="check">
            <input type="checkbox" checked={negativeSample} onChange={(e) => onNegativeSample(e.target.checked)} />
            否决时记为负样本（收紧该目标阈值）
          </label>
          <div className="btns">
            <button onClick={() => onTrack(s.id)}>向后延长 3 s</button>
          </div>
          <hr />
        </section>
      )}

      <section>
        <h4>{t.name}</h4>
        <div className="muted small">当前时刻生效 {visible} 段 · 共 {totalForTarget} 段</div>

        <label className="field">
          <span>名称</span>
          <input type="text" value={t.name} onChange={(e) => onUpdateTarget(t.id, { name: e.target.value })} />
        </label>

        <div className="field">
          <span>匹配模式</span>
          <div className="seg">
            {MODES.map((m) => (
              <button key={m.value} className={t.mode === m.value ? 'on' : ''} onClick={() => onUpdateTarget(t.id, { mode: m.value })}>
                {m.label}
              </button>
            ))}
          </div>
        </div>

        <label className="field">
          <span>阈值 <b>{t.threshold.toFixed(2)}</b></span>
          <input
            type="range"
            min={0.3}
            max={0.95}
            step={0.01}
            value={t.threshold}
            onChange={(e) => onUpdateTarget(t.id, { threshold: Number(e.target.value) })}
          />
        </label>

        {!t.enabled && <div className="notice">该目标未启用，不参与检测与打码。</div>}
        {t.enabled && totalForTarget === 0 && (
          <div className="notice">
            未检出。可以降低阈值，或换一种匹配模式。
            <div className="btns">
              <button className="sm" onClick={() => onUpdateTarget(t.id, { threshold: Math.max(0.3, Math.round((t.threshold - 0.1) * 100) / 100) })}>
                降低阈值 −0.1
              </button>
            </div>
          </div>
        )}

        <h5>打码</h5>
        <div className="seg">
          {KINDS.map((k) => (
            <button key={k.value} className={t.style.kind === k.value ? 'on' : ''} onClick={() => onUpdateStyle(t.id, { kind: k.value })}>
              {k.label}
            </button>
          ))}
        </div>
        <label className="field">
          <span>强度 <b>{t.style.strength.toFixed(2)}</b></span>
          <input type="range" min={0.1} max={1} step={0.05} value={t.style.strength} onChange={(e) => onUpdateStyle(t.id, { strength: Number(e.target.value) })} />
        </label>
        <label className="field">
          <span>外扩 <b>{t.style.padding} px</b></span>
          <input type="range" min={0} max={30} step={1} value={t.style.padding} onChange={(e) => onUpdateStyle(t.id, { padding: Number(e.target.value) })} />
        </label>
        <label className="check">
          <input type="checkbox" checked={t.style.feather} onChange={(e) => onUpdateStyle(t.id, { feather: e.target.checked })} />
          边缘羽化
        </label>
        <label className="check">
          <input type="checkbox" checked={t.style.motion_pad} onChange={(e) => onUpdateStyle(t.id, { motion_pad: e.target.checked })} />
          运动自适应外扩
        </label>
      </section>
    </aside>
  );
}
