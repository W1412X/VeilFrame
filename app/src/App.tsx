import { useEffect, useState } from 'react';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { Inspector } from './components/Inspector';
import { Preview } from './components/Preview';
import { Queue } from './components/Queue';
import { Sidebar } from './components/Sidebar';
import { Timeline } from './components/Timeline';
import { Icon } from './components/Icon';
import { ExportDialog, NewTargetDialog, type ExportOptions, type NewTargetInput } from './components/Dialogs';
import * as api from './api';
import type { BatchJob, ExportProgress, ScanProgress, Snapshot, ToolStatus } from './api';
import { formatTime, nextPending } from './domain/segments';
import type { Quad, Style, Target } from './domain/types';

type Dialog = 'export' | 'newTarget' | null;
type View = 'editor' | 'queue';

interface ScanState {
  targetId: string;
  index: number;
  total: number;
  t: number;
  duration: number;
}

const PALETTE = ['#d1495b', '#2a9d8f', '#8e6bbf', '#e09f3e', '#457b9d', '#3d9970'];

export function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [tools, setTools] = useState<ToolStatus | null>(null);
  const [time, setTime] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [selTarget, setSelTarget] = useState<string | null>(null);
  const [selSeg, setSelSeg] = useState<string | null>(null);
  const [onlyPending, setOnlyPending] = useState(false);
  const [compare, setCompare] = useState(false);
  const [negativeSample, setNegativeSample] = useState(true);
  const [roiTarget, setRoiTarget] = useState<string | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [scanState, setScanState] = useState<ScanState | null>(null);
  const [exportState, setExportState] = useState<ExportProgress | null>(null);
  const [view, setView] = useState<View>('editor');
  const [jobs, setJobs] = useState<BatchJob[]>([]);
  /** 当前编辑器里的项目来自哪个队列任务；普通打开时为 null。 */
  const [openedJobId, setOpenedJobId] = useState<string | null>(null);

  const project = snap?.project ?? null;
  const dur = project?.video?.duration ?? 0;
  const busy = scanState !== null || exportState !== null;

  const errText = (e: unknown): string =>
    e instanceof Error ? e.message : typeof e === 'string' ? e : JSON.stringify(e);

  /** 统一处理命令结果：成功后替换快照，失败时提示并返回 null。 */
  const apply = async (p: Promise<Snapshot>): Promise<Snapshot | null> => {
    try {
      const next = await p;
      setSnap(next);
      return next;
    } catch (e) {
      setMessage(errText(e));
      return null;
    }
  };

  const withFeedback = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      setMessage(errText(e));
    }
  };

  useEffect(() => {
    if (!api.isDesktop()) return;
    withFeedback(async () => {
      try {
        setSnap(await api.getSnapshot());
      } catch {
        // 尚未打开项目
      }
      setTools(await api.toolStatus());
      setJobs(await api.queueList());
    });
    let alive = true;
    let unScan: UnlistenFn | undefined;
    let unExport: UnlistenFn | undefined;
    let unQueue: UnlistenFn | undefined;
    api
      .onScanProgress((p: ScanProgress) =>
        setScanState({ targetId: p.target_id, index: p.target_index + 1, total: p.target_total, t: p.t, duration: p.duration }),
      )
      .then((u) => (alive ? (unScan = u) : u()));
    api
      .onExportProgress((p) => setExportState(p))
      .then((u) => (alive ? (unExport = u) : u()));
    api
      .onQueueChanged((list) => setJobs(list))
      .then((u) => (alive ? (unQueue = u) : u()));
    return () => {
      alive = false;
      unScan?.();
      unExport?.();
      unQueue?.();
    };
    // 事件只注册一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!message) return;
    const id = window.setTimeout(() => setMessage(null), 3000);
    return () => window.clearTimeout(id);
  }, [message]);

  useEffect(() => {
    if (playing && dur > 0 && time >= dur) setPlaying(false);
  }, [playing, time, dur]);

  const seek = (t: number) => setTime(Math.max(0, Math.min(dur || Number.MAX_SAFE_INTEGER, t)));

  const resetView = () => {
    setTime(0);
    setPlaying(false);
    setSelTarget(null);
    setSelSeg(null);
    setRoiTarget(null);
    setCompare(false);
  };

  const onOpenVideo = () =>
    withFeedback(async () => {
      if (busy) return;
      const path = await api.pickVideo();
      if (!path) return;
      const next = await apply(api.openVideo(path));
      if (!next) return;
      setOpenedJobId(null);
      resetView();
      setMessage('已打开视频。新建目标后点击扫描。');
    });

  const onOpenProject = () =>
    withFeedback(async () => {
      if (busy) return;
      const path = await api.pickProject();
      if (!path) return;
      const next = await apply(api.openProject(path));
      if (!next) return;
      setOpenedJobId(null);
      resetView();
      setMessage('已打开项目');
    });

  const onSaveProject = () =>
    withFeedback(async () => {
      if (!snap || busy) return;
      const path = snap.project_path ?? (await api.pickSavePath('项目.maskproj'));
      if (!path) return;
      const next = await apply(api.saveProject(path));
      if (next) setMessage('项目已保存');
    });

  const onUndo = () => {
    if (!snap?.can_undo || busy) return;
    void apply(api.undo());
  };
  const onRedo = () => {
    if (!snap?.can_redo || busy) return;
    void apply(api.redo());
  };

  const selectTarget = (id: string | null) => {
    setSelTarget(id);
    setSelSeg(null);
    if (!id) setRoiTarget(null);
  };

  const selectSegment = (id: string | null) => {
    setSelSeg(id);
    if (!id) return;
    const s = project?.segments.find((x) => x.id === id);
    if (s) setSelTarget(s.target);
  };

  const onAccept = (id: string) => {
    if (busy) return;
    void apply(api.acceptSegment(id));
  };
  const onReject = (id: string) => {
    if (busy) return;
    void withFeedback(async () => {
      const next = await apply(api.rejectSegment(id, negativeSample));
      if (!next) return;
      setSelSeg(null);
      if (negativeSample) setMessage('已否决，并记为负样本，该目标阈值 +0.01');
    });
  };
  const onTrack = (id: string) => {
    if (!busy) void apply(api.extendTrack(id));
  };
  const onDelete = (id: string) => {
    if (busy) return;
    void withFeedback(async () => {
      const next = await apply(api.deleteSegment(id));
      if (next) setSelSeg(null);
    });
  };
  const onEditQuad = (segId: string, t: number, quad: Quad) => {
    if (!busy) void apply(api.setKeyframe(segId, t, quad));
  };

  const patchTarget = (id: string, patch: Partial<Target>) => {
    const t = project?.targets.find((x) => x.id === id);
    if (!t || busy) return;
    void apply(api.updateTarget({ ...t, ...patch }));
  };
  const patchStyle = (id: string, patch: Partial<Style>) => {
    const t = project?.targets.find((x) => x.id === id);
    if (!t || busy) return;
    void apply(api.updateTarget({ ...t, style: { ...t.style, ...patch } }));
  };

  const onPickTemplate = (id: string) =>
    withFeedback(async () => {
      const path = await api.pickTemplate();
      const t = project?.targets.find((x) => x.id === id);
      if (!path || !t) return;
      await apply(api.updateTarget({ ...t, variants: [...t.variants, path] }));
    });

  const toggleRoiDrawing = (id: string) => {
    setRoiTarget((cur) => (cur === id ? null : id));
    setMessage(roiTarget === id ? null : '在画面上拖拽框选区域');
  };
  const onPickRoi = (roi: [number, number, number, number]) => {
    if (!roiTarget) return;
    const t = project?.targets.find((x) => x.id === roiTarget);
    setRoiTarget(null);
    if (!t) return;
    void apply(api.updateTarget({ ...t, roi }));
    setMessage('区域已保存');
  };

  const runScan = (ids: string[]) => {
    if (!project || busy) return;
    const targets = ids.filter((id) => project.targets.some((t) => t.id === id && t.enabled));
    if (targets.length === 0) {
      setMessage('没有启用的目标');
      return;
    }
    setPlaying(false);
    setScanState({ targetId: targets[0], index: 1, total: targets.length, t: 0, duration: dur });
    void withFeedback(async () => {
      try {
        const res = await api.scanTargets(targets);
        setSnap(res.snapshot);
        const s = res.summary;
        if (s.cancelled) setMessage(`扫描已取消，已扫到 ${s.scanned_until.toFixed(1)} s`);
        else if (s.cache_warning) setMessage(`扫描完成，共 ${s.segments} 段；${s.cache_warning}`);
        else setMessage(`扫描完成，共 ${s.segments} 段`);
      } finally {
        setScanState(null);
      }
    });
  };

  const onCreateTarget = (input: NewTargetInput) =>
    withFeedback(async () => {
      const id = `t${Date.now().toString(36)}`;
      const color = PALETTE[(project?.targets.length ?? 0) % PALETTE.length];
      const target: Target = {
        id,
        name: input.name,
        kind: input.kind,
        color,
        variants: input.template ? [input.template] : [],
        pattern: input.pattern ?? null,
        match_mode: input.matchMode,
        fuzzy_edits: 1,
        engine: input.kind === 'text' ? 'ocr' : input.kind === 'image' ? 'sift_ransac' : 'auto',
        mode: 'standard',
        threshold: 0.6,
        roi: null,
        style: { kind: 'mosaic', strength: 0.6, padding: 6, feather: true, motion_pad: true },
        enabled: true,
      };
      const next = await apply(api.addTarget(target));
      if (!next) return;
      setSelTarget(id);
      setSelSeg(null);
      setDialog(null);
      if (input.kind === 'region') {
        setRoiTarget(id);
        setMessage('在画面上拖拽框选区域，然后扫描');
      } else {
        setMessage(`已添加「${input.name}」，点击扫描开始检测`);
      }
    });

  const onExport = (opts: ExportOptions) =>
    withFeedback(async () => {
      if (!project?.video) return;
      setDialog(null);
      const output = await api.pickExportPath(exportName(project.video.path));
      if (!output) return;
      setExportState({ done: 0, total: 0 });
      try {
        const r = await api.exportVideo(output, opts.codec, opts.crf, opts.pendingPolicy === 'mask', openedJobId);
        setMessage(`导出完成：${r.frames} 帧，${r.masked_regions} 处打码`);
      } finally {
        setExportState(null);
      }
    });

  const onQueueAdd = () =>
    withFeedback(async () => {
      const paths = await api.pickVideos();
      if (paths.length === 0) return;
      const added = await api.queueAdd(paths);
      setJobs(await api.queueList());
      setMessage(added === 0 ? '这些视频已在队列中' : `已添加 ${added} 个视频`);
    });

  const onQueueStart = () =>
    withFeedback(async () => {
      setJobs(await api.queueStart());
    });

  const onQueueCancel = (job: BatchJob) =>
    withFeedback(async () => {
      await api.queueCancel(job.id);
      setJobs(await api.queueList());
    });

  const onQueueResume = (job: BatchJob) =>
    withFeedback(async () => {
      await api.queueResume(job.id);
      setJobs(await api.queueStart());
    });

  const onQueueRemove = (job: BatchJob) =>
    withFeedback(async () => {
      await api.queueRemove(job.id);
      setJobs(await api.queueList());
    });

  const onQueueOpen = (job: BatchJob) =>
    withFeedback(async () => {
      const next = await api.queueOpen(job.id);
      setSnap(next);
      setOpenedJobId(job.id);
      resetView();
      setView('editor');
      setMessage('已载入批量任务，请复核后导出');
    });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement;
      if (el.matches('input, select, textarea')) return;
      if (dialog) {
        if (e.key === 'Escape') setDialog(null);
        return;
      }
      if (view !== 'editor') return;
      const key = e.key.toLowerCase();
      if ((e.ctrlKey || e.metaKey) && key === 'z') {
        e.preventDefault();
        if (e.shiftKey) onRedo();
        else onUndo();
        return;
      }
      if (!project) return;
      switch (e.key) {
        case ' ':
          e.preventDefault();
          setPlaying((p) => !p);
          return;
        case 'ArrowLeft':
          e.preventDefault();
          seek(time - (e.shiftKey ? 1 : 1 / Math.max(1, project.video?.fps ?? 30)));
          return;
        case 'ArrowRight':
          e.preventDefault();
          seek(time + (e.shiftKey ? 1 : 1 / Math.max(1, project.video?.fps ?? 30)));
          return;
        case 'Enter':
          if (selSeg) onAccept(selSeg);
          return;
        case 'Backspace':
        case 'Delete':
          e.preventDefault();
          if (selSeg) onReject(selSeg);
          return;
        case '\\':
          setCompare((c) => !c);
          return;
      }
      if (key === 'f') {
        const next = nextPending(project, time);
        if (next) {
          selectSegment(next.id);
          seek(next.t0);
        } else setMessage('没有待确认段');
      } else if (key === 'e') {
        setDialog('export');
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  const pendingCount = project?.segments.filter((s) => s.status === 'pending' || s.status === 'low').length ?? 0;
  const desktop = api.isDesktop();
  const scanName = scanState ? project?.targets.find((t) => t.id === scanState.targetId)?.name ?? '目标' : '';

  return (
    <div className={`app ${project ? '' : 'empty'}`}>
      <header className="topbar">
        <div className="brand">VeilFrame</div>
        <div className="muted small path">{snap?.project_path ?? project?.video?.path ?? '未打开视频'}</div>
        {project && (
          <div className="tabs">
            <button className={view === 'editor' ? 'on' : ''} onClick={() => setView('editor')}>
              编辑器
            </button>
            <button className={view === 'queue' ? 'on' : ''} onClick={() => setView('queue')}>
              批量队列{jobs.length > 0 ? ` (${jobs.length})` : ''}
            </button>
          </div>
        )}
        <span className="sp" />
        {!desktop && <span className="muted small">浏览器预览</span>}
        {desktop && tools && !tools.ffmpeg && <span className="warn-text" title="需要 ffmpeg 与 ffprobe：放入随包 bin 目录或加入 PATH">缺少 ffmpeg</span>}
        {desktop && tools && !tools.ocr_models && <span className="warn-text" title="文本目标需要 PP-OCRv5 模型与 ONNX Runtime">OCR 模型未就位</span>}
        {snap?.autosave_error && <span className="warn-text" title={snap.autosave_error}>自动保存失败</span>}
        <button className="ghost" title="打开视频" disabled={!desktop || busy} onClick={onOpenVideo}>
          <Icon name="videoOpen" />
        </button>
        <button className="ghost" title="打开项目" disabled={!desktop || busy} onClick={onOpenProject}>
          <Icon name="folderOpen" />
        </button>
        {view === 'editor' && (
          <>
            <button className="ghost" title="保存项目" disabled={!project || busy} onClick={onSaveProject}>
              <Icon name="save" />
            </button>
            <button className="ghost" title="撤销 (Ctrl+Z)" disabled={!snap?.can_undo || busy} onClick={onUndo}>
              <Icon name="undo" />
            </button>
            <button className="ghost" title="重做 (Ctrl+Shift+Z)" disabled={!snap?.can_redo || busy} onClick={onRedo}>
              <Icon name="redo" />
            </button>
            <button
              className="ghost"
              title="扫描所有启用的目标"
              disabled={!project || busy || project.targets.every((t) => !t.enabled)}
              onClick={() => runScan(project?.targets.filter((t) => t.enabled).map((t) => t.id) ?? [])}
            >
              <Icon name="scan" />
            </button>
            <button
              className="ghost"
              title="快捷键"
              onClick={() =>
                setMessage('Space 播放 · ←→ 逐帧 · F 下一待确认 · Enter 接受 · Delete 否决 · 拖动四角改形状 · \\ 对比原画 · E 导出 · Ctrl+Z 撤销')
              }
            >
              <Icon name="keyboard" />
            </button>
            <button className="primary" disabled={!project?.video || busy} onClick={() => setDialog('export')}>
              <Icon name="export" /> 导出
            </button>
          </>
        )}
      </header>

      {!project ? (
        <div className="welcome">
          <h2>本地离线视频打码</h2>
          <p className="muted">打开视频 → 新建目标 → 扫描 → 复核 → 导出。所有处理都在本机完成。</p>
          <div className="btns">
            <button className="primary" disabled={!desktop} onClick={onOpenVideo}>
              <Icon name="videoOpen" /> 打开视频
            </button>
            <button disabled={!desktop} onClick={onOpenProject}>
              <Icon name="folderOpen" /> 打开项目
            </button>
          </div>
          {!desktop && <p className="muted small">当前是浏览器预览，请从桌面应用打开。</p>}
        </div>
      ) : view === 'queue' ? (
        <div className="queue-wrap">
          <Queue
            jobs={jobs}
            onAdd={onQueueAdd}
            onStart={onQueueStart}
            onOpen={onQueueOpen}
            onCancel={onQueueCancel}
            onResume={onQueueResume}
            onRemove={onQueueRemove}
          />
        </div>
      ) : (
        <>
          <div className="body">
            <Sidebar
              project={project}
              selTarget={selTarget}
              busy={busy}
              scanningTargetId={scanState?.targetId ?? null}
              onSelectTarget={selectTarget}
              onToggleTarget={(id, enabled) => patchTarget(id, { enabled })}
              onNewTarget={() => setDialog('newTarget')}
              onScanAll={() => runScan(project.targets.filter((t) => t.enabled).map((t) => t.id))}
            />

            <section className="center">
              <div className="stage">
                <Preview
                  project={project}
                  time={time}
                  playing={playing}
                  selTarget={selTarget}
                  selSeg={selSeg}
                  compare={compare}
                  roiTargetId={roiTarget}
                  onTime={setTime}
                  onPlayingChange={setPlaying}
                  onSelectSeg={selectSegment}
                  onEditQuad={onEditQuad}
                  onPickRoi={onPickRoi}
                />
                {compare && <div className="orig-badge">原画对比中 · \ 切换回</div>}
                {roiTarget && <div className="orig-badge draw-hint">拖拽画面框选区域</div>}
              </div>
              <div className="transport">
                <button className="icon-btn" title="回到开头" onClick={() => seek(0)}>
                  <Icon name="skipPrevious" />
                </button>
                <button className="icon-btn" title="上一帧 (←)" onClick={() => seek(time - 1 / Math.max(1, project.video?.fps ?? 30))}>
                  <Icon name="stepBackward" />
                </button>
                <button className="icon-btn primary" title="播放 (Space)" onClick={() => setPlaying((p) => !p)}>
                  <Icon name={playing ? 'pause' : 'play'} />
                </button>
                <button className="icon-btn" title="下一帧 (→)" onClick={() => seek(time + 1 / Math.max(1, project.video?.fps ?? 30))}>
                  <Icon name="stepForward" />
                </button>
                <span className="time">
                  {formatTime(time)} / {formatTime(dur)}
                </span>
                <span className="sp" />
                <button className={compare ? 'on' : ''} onClick={() => setCompare((c) => !c)}>
                  对比原画 \
                </button>
                <span className="muted small">{pendingCount} 段待确认</span>
              </div>
            </section>

            <Inspector
              project={project}
              selTarget={selTarget}
              selSeg={selSeg}
              negativeSample={negativeSample}
              busy={busy}
              ocrReady={!tools || tools.ocr_models}
              roiDrawing={roiTarget === selTarget}
              onNegativeSample={setNegativeSample}
              onAccept={onAccept}
              onReject={onReject}
              onTrack={onTrack}
              onDelete={onDelete}
              onUpdateTarget={patchTarget}
              onUpdateStyle={patchStyle}
              onScanTarget={(id) => runScan([id])}
              onPickTemplate={onPickTemplate}
              onDrawRoi={toggleRoiDrawing}
              onClearRoi={(id) => patchTarget(id, { roi: null })}
              time={time}
            />
          </div>

          <Timeline
            project={project}
            time={time}
            selSeg={selSeg}
            selTarget={selTarget}
            onlyPending={onlyPending}
            onTogglePending={() => setOnlyPending((v) => !v)}
            onSeek={seek}
            onSelectSeg={selectSegment}
            onSelectTarget={selectTarget}
          />
        </>
      )}

      {scanState && (
        <div className="jobbar">
          <div className="jobbar-row">
            <b>
              扫描「{scanName}」{scanState.total > 1 ? ` ${scanState.index}/${scanState.total}` : ''}
            </b>
            <span>{percent(scanState.t, scanState.duration)}</span>
          </div>
          <button className="sm ghost" onClick={() => void api.cancelScan()}>
            取消
          </button>
          <div className="bar">
            <i style={{ width: percent(scanState.t, scanState.duration) }} />
          </div>
          <div className="muted small">
            {scanState.t.toFixed(1)} / {scanState.duration.toFixed(1)} s
          </div>
        </div>
      )}

      {exportState && (
        <div className="jobbar">
          <div className="jobbar-row">
            <b>导出中</b>
            <span>{percent(exportState.done, exportState.total)}</span>
          </div>
          <button className="sm ghost" onClick={() => void api.cancelExport()}>
            取消
          </button>
          <div className="bar">
            <i style={{ width: percent(exportState.done, exportState.total) }} />
          </div>
          {exportState.total > 0 && (
            <div className="muted small">
              {exportState.done} / {exportState.total} 帧
            </div>
          )}
        </div>
      )}

      {dialog === 'export' && project && (
        <ExportDialog pendingCount={pendingCount} onCancel={() => setDialog(null)} onExport={onExport} />
      )}
      {dialog === 'newTarget' && (
        <NewTargetDialog onCancel={() => setDialog(null)} onCreate={onCreateTarget} pickTemplate={api.pickTemplate} />
      )}

      {message && <div className="toast">{message}</div>}
    </div>
  );
}

function percent(done: number, total: number): string {
  if (total <= 0) return '0%';
  return `${Math.min(100, Math.round((done / total) * 100))}%`;
}

function exportName(videoPath: string): string {
  const base = videoPath.split(/[\\/]/).pop() ?? 'video.mp4';
  const dot = base.lastIndexOf('.');
  return `${dot > 0 ? base.slice(0, dot) : base}_masked.mp4`;
}
