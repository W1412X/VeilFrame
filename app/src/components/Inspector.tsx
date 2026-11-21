import { useEffect, useRef, useState } from 'react';
import type { MaskKind, Mode, Project, Style, Target } from '../domain/types';
import { activeSegments } from '../domain/segments';

interface Props {
  project: Project;
  selTarget: string | null;
  selSeg: string | null;
  negativeSample: boolean;
  busy: boolean;
  ocrReady: boolean;
  roiDrawing: boolean;
  onNegativeSample: (v: boolean) => void;
  onAccept: (id: string) => void;
  onReject: (id: string) => void;
  onTrack: (id: string) => void;
  onDelete: (id: string) => void;
  onUpdateTarget: (id: string, patch: Partial<Target>) => void;
  onUpdateStyle: (id: string, patch: Partial<Style>) => void;
  onScanTarget: (id: string) => void;
  onPickTemplate: (id: string) => void;
  onDrawRoi: (id: string) => void;
  onClearRoi: (id: string) => void;
  time: number;
}

const MODES: { value: Mode; label: string }[] = [
  { value: 'fast', label: '极速' },
  { value: 'standard', label: '标准' },
  { value: 'precise', label: '精准' },
];

const MATCH_MODES: { value: Target['match_mode']; label: string }[] = [
  { value: 'contains', label: '包含' },
  { value: 'exact', label: '精确' },
  { value: 'regex', label: '正则' },
  { value: 'fuzzy', label: '模糊' },
];

const KINDS: { value: MaskKind; label: string }[] = [
  { value: 'mosaic', label: '马赛克' },
  { value: 'blur', label: '模糊' },
  { value: 'solid', label: '色块' },
];

const STATUS_LABEL = { confirmed: '已确认', pending: '待确认', low: '低置信', rejected: '已否决' };

export function Inspector(props: Props) {
  const {
    project, selTarget, selSeg, negativeSample, busy, ocrReady, roiDrawing,
    onNegativeSample, onAccept, onReject, onTrack, onDelete,
    onUpdateTarget, onUpdateStyle, onScanTarget, onPickTemplate, onDrawRoi, onClearRoi, time,
  } = props;
  const t = project.targets.find((x) => x.id === selTarget);
  const s = project.segments.find((x) => x.id === selSeg);
  if (!t) return <aside className="inspector"><p className="muted">选择左侧目标，或画面中的检测段。</p></aside>;

  const visible = activeSegments(project, time).filter((x) => x.target === t.id).length;
  const totalForTarget = project.segments.filter((x) => x.target === t.id && x.status !== 'rejected').length;
  const patch = (p: Partial<Target>) => onUpdateTarget(t.id, p);
  const roiLabel = t.roi ? `${t.roi[2]}×${t.roi[3]} @ ${t.roi[0]},${t.roi[1]}` : '整帧';

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
            <button className="primary" disabled={busy || s.status === 'confirmed'} onClick={() => onAccept(s.id)}>接受</button>
            <button disabled={busy || s.status === 'rejected'} onClick={() => onReject(s.id)}>否决</button>
            <button className="ghost" disabled={busy} onClick={() => onDelete(s.id)}>删除</button>
          </div>
          <label className="check">
            <input type="checkbox" checked={negativeSample} onChange={(e) => onNegativeSample(e.target.checked)} />
            否决时记为负样本（收紧该目标阈值）
          </label>
          <div className="btns">
            <button disabled={busy} onClick={() => onTrack(s.id)}>向后延长 3 s</button>
          </div>
          <hr />
        </section>
      )}

      <section>
        <h4>{t.name}</h4>
        <div className="muted small">当前时刻生效 {visible} 段 · 共 {totalForTarget} 段</div>

        <TextInput label="名称" value={t.name} disabled={busy} onCommit={(v) => v.trim() && patch({ name: v.trim() })} />

        {t.kind === 'text' && (
          <>
            <TextInput
              label="匹配串"
              value={t.pattern ?? ''}
              placeholder="要打码的文字"
              disabled={busy}
              onCommit={(v) => v.trim() && patch({ pattern: v })}
            />
            <div className="field">
              <span>匹配方式</span>
              <div className="seg">
                {MATCH_MODES.map((m) => (
                  <button key={m.value} className={t.match_mode === m.value ? 'on' : ''} disabled={busy} onClick={() => patch({ match_mode: m.value })}>
                    {m.label}
                  </button>
                ))}
              </div>
            </div>
            {t.match_mode === 'fuzzy' && (
              <TextInput
                label="允许编辑距离"
                value={String(t.fuzzy_edits)}
                disabled={busy}
                onCommit={(v) => {
                  const n = Number(v);
                  if (Number.isInteger(n) && n > 0) patch({ fuzzy_edits: n });
                }}
              />
            )}
            {!ocrReady && <div className="notice">OCR 模型或 ONNX Runtime 未就位，文本目标无法扫描。请把模型放到用户数据目录或随包资源。</div>}
          </>
        )}

        {t.kind === 'image' && (
          <>
            <div className="field">
              <span>模板图 <b>{t.variants.length} 个</b></span>
              {t.variants.length === 0 && <div className="muted small">还没有模板图，扫描前必须先选择。</div>}
              {t.variants.map((v, i) => (
                <div key={`${v}-${i}`} className="file-row" title={v}>
                  <span>{basename(v)}</span>
                  <button
                    className="ghost sm"
                    disabled={busy}
                    onClick={() => patch({ variants: t.variants.filter((_, j) => j !== i) })}
                    title="移除模板"
                  >
                    ×
                  </button>
                </div>
              ))}
              <div className="btns">
                <button className="sm" disabled={busy} onClick={() => onPickTemplate(t.id)}>选择模板图…</button>
              </div>
            </div>
            <div className="field">
              <span>匹配模式</span>
              <div className="seg">
                {MODES.map((m) => (
                  <button key={m.value} className={t.mode === m.value ? 'on' : ''} disabled={busy} onClick={() => patch({ mode: m.value })}>
                    {m.label}
                  </button>
                ))}
              </div>
              <div className="muted small">{t.mode === 'fast' ? 'NCC 模板匹配，适合近静态台标。' : 'SIFT + RANSAC，适合透视与缩放。'}</div>
            </div>
          </>
        )}

        {t.kind === 'region' && (
          <div className="field">
            <span>{roiDrawing ? '在画面上拖拽框选' : `区域 ${roiLabel}`}</span>
            <div className="btns">
              <button className={roiDrawing ? 'on' : ''} disabled={busy} onClick={() => onDrawRoi(t.id)}>
                {roiDrawing ? '框选中…（拖拽画面）' : '框选区域'}
              </button>
              {t.roi && <button className="ghost" disabled={busy} onClick={() => onClearRoi(t.id)}>整帧</button>}
            </div>
            <div className="muted small">区域目标扫描后生成覆盖全片的一段，框即打码范围。</div>
          </div>
        )}

        {(t.kind === 'text' || t.kind === 'image') && (
          <div className="field">
            <span>搜索区域 <b>{roiLabel}</b></span>
            <div className="btns">
              <button className={roiDrawing ? 'on' : ''} disabled={busy} onClick={() => onDrawRoi(t.id)}>
                {roiDrawing ? '框选中…（拖拽画面）' : '限定区域'}
              </button>
              {t.roi && <button className="ghost" disabled={busy} onClick={() => onClearRoi(t.id)}>整帧</button>}
            </div>
          </div>
        )}

        <Range
          label="阈值"
          min={0.3}
          max={0.95}
          step={0.01}
          value={t.threshold}
          format={(v) => v.toFixed(2)}
          disabled={busy}
          onCommit={(v) => patch({ threshold: v })}
        />

        <div className="btns">
          <button className="primary" disabled={busy || !t.enabled} onClick={() => onScanTarget(t.id)}>
            {busy ? '处理中…' : '扫描此目标'}
          </button>
        </div>

        {!t.enabled && <div className="notice">该目标未启用，不参与检测与打码。</div>}
        {t.enabled && totalForTarget === 0 && t.kind !== 'region' && (
          <div className="notice">
            未检出。可以降低阈值，或换一种匹配模式。
            <div className="btns">
              <button className="sm" disabled={busy} onClick={() => patch({ threshold: Math.max(0.3, Math.round((t.threshold - 0.1) * 100) / 100) })}>
                降低阈值 −0.1
              </button>
            </div>
          </div>
        )}

        <h5>打码</h5>
        <div className="seg">
          {KINDS.map((k) => (
            <button key={k.value} className={t.style.kind === k.value ? 'on' : ''} disabled={busy} onClick={() => onUpdateStyle(t.id, { kind: k.value })}>
              {k.label}
            </button>
          ))}
        </div>
        <Range
          label="强度"
          min={0.1}
          max={1}
          step={0.05}
          value={t.style.strength}
          format={(v) => v.toFixed(2)}
          disabled={busy}
          onCommit={(v) => onUpdateStyle(t.id, { strength: v })}
        />
        <Range
          label="外扩"
          min={0}
          max={30}
          step={1}
          value={t.style.padding}
          format={(v) => `${v} px`}
          disabled={busy}
          onCommit={(v) => onUpdateStyle(t.id, { padding: v })}
        />
        <label className="check">
          <input type="checkbox" checked={t.style.feather} disabled={busy} onChange={(e) => onUpdateStyle(t.id, { feather: e.target.checked })} />
          边缘羽化
        </label>
        <label className="check">
          <input type="checkbox" checked={t.style.motion_pad} disabled={busy} onChange={(e) => onUpdateStyle(t.id, { motion_pad: e.target.checked })} />
          运动自适应外扩
        </label>
      </section>
    </aside>
  );
}

function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

interface TextInputProps {
  label: string;
  value: string;
  placeholder?: string;
  disabled?: boolean;
  onCommit: (value: string) => void;
}

/** 失焦或回车才提交，避免每敲一个字都写入项目。 */
function TextInput({ label, value, placeholder, disabled, onCommit }: TextInputProps) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (draft !== value) onCommit(draft);
  };
  return (
    <label className="field">
      <span>{label}</span>
      <input
        type="text"
        value={draft}
        placeholder={placeholder}
        disabled={disabled}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur();
        }}
      />
    </label>
  );
}

interface RangeProps {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  format: (v: number) => string;
  disabled?: boolean;
  onCommit: (value: number) => void;
}

/** 拖动时只更新本地显示，松手/键盘调整后才提交。 */
function Range({ label, min, max, step, value, format, disabled, onCommit }: RangeProps) {
  const [draft, setDraft] = useState(value);
  const current = useRef(value);
  useEffect(() => {
    current.current = value;
    setDraft(value);
  }, [value]);
  const commit = () => {
    if (current.current !== value) onCommit(current.current);
  };
  return (
    <label className="field">
      <span>{label} <b>{format(draft)}</b></span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={draft}
        disabled={disabled}
        onChange={(e) => {
          const v = Number(e.target.value);
          current.current = v;
          setDraft(v);
        }}
        onPointerUp={commit}
        onKeyUp={commit}
        onBlur={commit}
      />
    </label>
  );
}
