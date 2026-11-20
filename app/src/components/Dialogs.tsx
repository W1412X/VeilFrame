import { useState, type ReactNode } from 'react';
import type { TargetKind } from '../domain/types';

interface ExportProps {
  pendingCount: number;
  onCancel: () => void;
  onExport: (options: ExportOptions) => void;
}

export interface ExportOptions {
  codec: 'libx264' | 'h264_nvenc' | 'h264_qsv' | 'h264_amf';
  crf: number;
  pendingPolicy: 'mask' | 'confirmedOnly';
  audio: 'copy';
}

/** 导出对话框。选项会在导出命令接入后端时使用。 */
export function ExportDialog({ pendingCount, onCancel, onExport }: ExportProps) {
  const [codec, setCodec] = useState<ExportOptions['codec']>('libx264');
  const [crf, setCrf] = useState(18);
  const [pendingPolicy, setPendingPolicy] = useState<ExportOptions['pendingPolicy']>('mask');

  return (
    <Dialog title="导出" onClose={onCancel}>
      <Field label="编码器">
        <select value={codec} onChange={(e) => setCodec(e.target.value as ExportOptions['codec'])}>
          <option value="libx264">x264 软编（兜底）</option>
          <option value="h264_nvenc">NVENC（需 NVIDIA 驱动）</option>
          <option value="h264_qsv">QSV（Intel）</option>
          <option value="h264_amf">AMF（AMD）</option>
        </select>
      </Field>
      <Field label={`质量 CRF ${crf}`}>
        <input type="range" min={14} max={30} step={1} value={crf} onChange={(e) => setCrf(Number(e.target.value))} />
      </Field>
      <Field label="待确认段">
        <div className="seg">
          <button className={pendingPolicy === 'mask' ? 'on' : ''} onClick={() => setPendingPolicy('mask')}>
            按打码处理（默认）
          </button>
          <button className={pendingPolicy === 'confirmedOnly' ? 'on' : ''} onClick={() => setPendingPolicy('confirmedOnly')}>
            仅已确认
          </button>
        </div>
        <div className="muted small">当前仍有 {pendingCount} 段待确认。</div>
      </Field>
      <Field label="音频">
        <span className="muted small">默认直接拷贝（copy），不重编码</span>
      </Field>
      <div className="foot">
        <button onClick={onCancel}>取消</button>
        <button className="primary" onClick={() => onExport({ codec, crf, pendingPolicy, audio: 'copy' })}>
          导出
        </button>
      </div>
    </Dialog>
  );
}

interface NewTargetProps {
  onCancel: () => void;
  onCreate: (name: string, kind: TargetKind) => void;
}

const KIND_OPTIONS: { value: TargetKind; label: string }[] = [
  { value: 'text', label: '文本' },
  { value: 'image', label: '图像' },
  { value: 'region', label: '固定区域' },
];

export function NewTargetDialog({ onCancel, onCreate }: NewTargetProps) {
  const [name, setName] = useState('');
  const [kind, setKind] = useState<TargetKind>('text');
  const canCreate = name.trim().length > 0;

  return (
    <Dialog title="新建目标" onClose={onCancel}>
      <Field label="名称">
        <input type="text" autoFocus value={name} placeholder="例如：机密" onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field label="类型">
        <select value={kind} onChange={(e) => setKind(e.target.value as TargetKind)}>
          {KIND_OPTIONS.map((k) => (
            <option key={k.value} value={k.value}>
              {k.label}
            </option>
          ))}
        </select>
      </Field>
      <div className="foot">
        <button onClick={onCancel}>取消</button>
        <button className="primary" disabled={!canCreate} onClick={() => onCreate(name.trim(), kind)}>
          添加
        </button>
      </div>
    </Dialog>
  );
}

function Dialog({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  return (
    <div
      className="modal-mask"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="dialog" role="dialog" aria-label={title}>
        <h3>{title}</h3>
        {children}
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="field">
      <span>{label}</span>
      {children}
    </label>
  );
}
