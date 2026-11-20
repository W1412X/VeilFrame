import { useEffect, useState } from 'react';
import { Inspector } from './components/Inspector';
import { Preview } from './components/Preview';
import { Sidebar } from './components/Sidebar';
import { Timeline } from './components/Timeline';
import { Icon } from './components/Icon';
import { ExportDialog, NewTargetDialog, type ExportOptions } from './components/Dialogs';
import { Queue, SAMPLE_JOBS, type Job } from './components/Queue';
import {
  addTarget,
  createEditor,
  deleteSegment,
  extendTrack,
  redo,
  rejectSegment,
  setKeyframeQuad,
  setSegmentStatus,
  undo,
  updateTarget,
  type EditorState,
} from './domain/editor';
import { sampleProject } from './domain/sample';
import { isDesktop, loadProject, newProject, pickProject, pickSavePath, pickVideo, saveProject } from './api';
import type { Quad, Style, Target, TargetKind } from './domain/types';

type View = 'work' | 'queue';
type Dialog = 'export' | 'newTarget' | null;

const PALETTE = ['#d1495b', '#2a9d8f', '#8e6bbf', '#e09f3e', '#457b9d', '#3d9970'];

export function App() {
  const [ed, setEd] = useState<EditorState>(() => createEditor(sampleProject));
  const [time, setTime] = useState(12.3);
  const [playing, setPlaying] = useState(false);
  const [selTarget, setSelTarget] = useState<string | null>('t1');
  const [selSeg, setSelSeg] = useState<string | null>(null);
  const [onlyPending, setOnlyPending] = useState(false);
  const [compare, setCompare] = useState(false);
  const [negativeSample, setNegativeSample] = useState(true);
  const [message, setMessage] = useState<string | null>(null);
  const [view, setView] = useState<View>('work');
  const [dialog, setDialog] = useState<Dialog>(null);
  const [jobs] = useState<Job[]>(SAMPLE_JOBS);
  const [projectPath, setProjectPath] = useState<string | null>(null);

  /** 桌面端文件操作：失败或浏览器预览时给出提示，不抛到界面外。 */
  const withFeedback = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      setMessage(e instanceof Error ? e.message : String(e));
    }
  };

  const resetSelection = () => {
    setSelTarget(null);
    setSelSeg(null);
    setTime(0);
    setPlaying(false);
  };

  const onOpenVideo = () =>
    withFeedback(async () => {
      const path = await pickVideo();
      if (!path) return;
      setEd(createEditor(await newProject(path)));
      setProjectPath(null);
      resetSelection();
      setMessage('已打开视频。当前没有检测结果，扫描接入后端后显示。');
    });

  const onOpenProject = () =>
    withFeedback(async () => {
      const path = await pickProject();
      if (!path) return;
      setEd(createEditor(await loadProject(path)));
      setProjectPath(path);
      resetSelection();
      setMessage('已打开项目');
    });

  const onSaveProject = () =>
    withFeedback(async () => {
      const path = projectPath ?? (await pickSavePath('项目.maskproj'));
      if (!path) return;
      await saveProject(path, ed.project);
      setProjectPath(path);
      setMessage('项目已保存');
    });

  const project = ed.project;
  const dur = project.video?.duration ?? 0;
  const pendingCount = project.segments.filter((s) => s.status === 'pending' || s.status === 'low').length;

  // 播放：每 100 ms 前进一步
  useEffect(() => {
    if (!playing) return;
    const id = window.setInterval(() => setTime((t) => Math.min(dur, t + 0.1)), 100);
    return () => window.clearInterval(id);
  }, [playing, dur]);

  useEffect(() => {
    if (playing && time >= dur) setPlaying(false);
  }, [playing, time, dur]);

  // 提示 2.5 秒后消失
  useEffect(() => {
    if (!message) return;
    const id = window.setTimeout(() => setMessage(null), 2500);
    return () => window.clearTimeout(id);
  }, [message]);

  const seek = (t: number) => setTime(Math.max(0, Math.min(dur, t)));

  /** 执行一次编辑；校验失败时保留原状态并提示。 */
  const run = (fn: (s: EditorState) => EditorState, after?: () => void) => {
    try {
      setEd(fn(ed));
      after?.();
    } catch (e) {
      setMessage(e instanceof Error ? e.message : String(e));
    }
  };

  const selectSegment = (id: string | null) => {
    setSelSeg(id);
    if (!id) return;
    const s = project.segments.find((x) => x.id === id);
    if (s) setSelTarget(s.target);
  };

  const onAccept = (id: string) => run((s) => setSegmentStatus(s, id, 'confirmed'));
  const onReject = (id: string) =>
    run(
      (s) => rejectSegment(s, id, negativeSample),
      () => {
        setSelSeg(null);
        if (negativeSample) setMessage('已否决，并记为负样本，该目标阈值 +0.01');
      },
    );
  const onTrack = (id: string) => run((s) => extendTrack(s, id));
  const onDelete = (id: string) =>
    run(
      (s) => deleteSegment(s, id),
      () => setSelSeg(null),
    );
  const onEditQuad = (segId: string, t: number, quad: Quad) => run((s) => setKeyframeQuad(s, segId, t, quad));
  const onUpdateTarget = (id: string, patch: Partial<Target>) => run((s) => updateTarget(s, id, patch));
  const onUpdateStyle = (id: string, patch: Partial<Style>) => {
    const tg = project.targets.find((x) => x.id === id);
    if (!tg) return;
    run((s) => updateTarget(s, id, { style: { ...tg.style, ...patch } }));
  };
  const onToggleTarget = (id: string, enabled: boolean) => run((s) => updateTarget(s, id, { enabled }));

  const onCreateTarget = (name: string, kind: TargetKind) => {
    const id = `t${Date.now().toString(36)}`;
    const color = PALETTE[project.targets.length % PALETTE.length];
    const target: Target = {
      id,
      name,
      kind,
      color,
      engine: kind === 'image' ? 'sift_ransac' : 'ocr',
      mode: 'standard',
      threshold: 0.6,
      style: { kind: 'mosaic', strength: 0.6, padding: 6, feather: true, motion_pad: true },
      enabled: true,
    };
    run(
      (s) => addTarget(s, target),
      () => {
        setSelTarget(id);
        setSelSeg(null);
        setDialog(null);
        setMessage(`已添加「${name}」，暂无检出段（扫描接入后端后显示结果）`);
      },
    );
  };

  // 键盘快捷键
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement;
      if (el.matches('input, select, textarea')) return;
      if (dialog) {
        if (e.key === 'Escape') setDialog(null);
        return;
      }
      const key = e.key.toLowerCase();
      if ((e.ctrlKey || e.metaKey) && key === 'z') {
        e.preventDefault();
        setEd(e.shiftKey ? redo : undo);
        return;
      }
      if (view !== 'work') return;
      switch (e.key) {
        case ' ':
          e.preventDefault();
          setPlaying((p) => !p);
          return;
        case 'ArrowLeft':
          e.preventDefault();
          seek(time - (e.shiftKey ? 1 : 1 / 30));
          return;
        case 'ArrowRight':
          e.preventDefault();
          seek(time + (e.shiftKey ? 1 : 1 / 30));
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
        const next = pendingAfter(project, time);
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

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">VeilFrame</div>
        <nav className="tabs">
          <button className={view === 'work' ? 'on' : ''} onClick={() => setView('work')}>
            工作台
          </button>
          <button className={view === 'queue' ? 'on' : ''} onClick={() => setView('queue')}>
            批量队列
          </button>
        </nav>
        <div className="muted small">{projectPath ?? project.video?.path ?? '未打开视频'}</div>
        <span className="sp" />
        <button className="ghost" title="打开视频" onClick={onOpenVideo}>
          <Icon name="videoOpen" />
        </button>
        <button className="ghost" title="打开项目" onClick={onOpenProject}>
          <Icon name="folderOpen" />
        </button>
        <button className="ghost" title="保存项目" onClick={onSaveProject}>
          <Icon name="save" />
        </button>
        {!isDesktop() && <span className="muted small">浏览器预览</span>}
        <button className="ghost" title="撤销 (Ctrl+Z)" disabled={ed.past.length === 0} onClick={() => setEd(undo)}>
          <Icon name="undo" />
        </button>
        <button className="ghost" title="重做 (Ctrl+Shift+Z)" disabled={ed.future.length === 0} onClick={() => setEd(redo)}>
          <Icon name="redo" />
        </button>
        <button
          className="ghost"
          title="快捷键"
          onClick={() =>
            setMessage('Space 播放 · ←→ 逐帧 · F 下一待确认 · Enter 接受 · Delete 否决 · 拖动四角修改形状 · \\ 对比原画 · E 导出 · Ctrl+Z 撤销')
          }
        >
          <Icon name="keyboard" />
        </button>
        <button className="primary" onClick={() => setDialog('export')}>
          <Icon name="export" /> 导出
        </button>
      </header>

      {view === 'work' ? (
        <>
          <div className="body">
            <Sidebar
              project={project}
              selTarget={selTarget}
              onSelectTarget={(id) => {
                setSelTarget(id);
                setSelSeg(null);
              }}
              onToggleTarget={onToggleTarget}
              onNewTarget={() => setDialog('newTarget')}
            />

            <section className="center">
              <div className="stage">
                <Preview
                  project={project}
                  time={time}
                  selSeg={selSeg}
                  compare={compare}
                  onSelectSeg={selectSegment}
                  onEditQuad={onEditQuad}
                />
                {compare && <div className="orig-badge">原画对比中 · \ 切换回</div>}
              </div>
              <div className="transport">
                <button className="icon-btn" title="回到开头" onClick={() => seek(0)}>
                  <Icon name="skipPrevious" />
                </button>
                <button className="icon-btn" title="上一帧 (←)" onClick={() => seek(time - 1 / 30)}>
                  <Icon name="stepBackward" />
                </button>
                <button className="icon-btn primary" title="播放 (Space)" onClick={() => setPlaying((p) => !p)}>
                  <Icon name={playing ? 'pause' : 'play'} />
                </button>
                <button className="icon-btn" title="下一帧 (→)" onClick={() => seek(time + 1 / 30)}>
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
              onNegativeSample={setNegativeSample}
              onAccept={onAccept}
              onReject={onReject}
              onTrack={onTrack}
              onDelete={onDelete}
              onUpdateTarget={onUpdateTarget}
              onUpdateStyle={onUpdateStyle}
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
            onSelectTarget={(id) => {
              setSelTarget(id);
              setSelSeg(null);
            }}
          />
        </>
      ) : (
        <Queue
          jobs={jobs}
          onAdd={() => setMessage('拖入视频添加任务（需要后端文件选择）')}
          onAction={(job) => {
            if (job.state === 'review') {
              setView('work');
              return;
            }
            setMessage(`「${job.file}」的操作需要后端扫描引擎（下一步接入）`);
          }}
        />
      )}

      {dialog === 'export' && (
        <ExportDialog
          pendingCount={pendingCount}
          onCancel={() => setDialog(null)}
          onExport={(opts: ExportOptions) => {
            setDialog(null);
            setMessage(`导出参数已记录（${opts.codec}, CRF ${opts.crf}），导出命令需要后端接入`);
          }}
        />
      )}
      {dialog === 'newTarget' && <NewTargetDialog onCancel={() => setDialog(null)} onCreate={onCreateTarget} />}

      {message && <div className="toast">{message}</div>}
    </div>
  );
}

function pendingAfter(p: EditorState['project'], t: number) {
  const list = p.segments
    .filter((s) => s.status === 'pending' || s.status === 'low')
    .sort((a, b) => a.t0 - b.t0);
  if (list.length === 0) return undefined;
  return list.find((s) => s.t0 > t + 0.01) ?? list[0];
}

function formatTime(t: number): string {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  return `${String(m).padStart(2, '0')}:${s.toFixed(2).padStart(5, '0')}`;
}
