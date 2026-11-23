import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type UIEvent,
} from 'react';
import type { Project } from '../domain/types';
import { formatTime, nextPending } from '../domain/segments';

// 时间轴交互说明（与 docs/preview/timeline-interaction.html 一致）：
// - 标尺与播放头把手是拖动入口；按下即擦洗，拖动时显示时间气泡；轨道空白处按下也会跳转。
// - 单击段选中并跳到段首，双击段放大到该段。
// - Ctrl/⌘ + 滚轮以鼠标位置为中心缩放；Shift + 滚轮或触控板横向滚动。
// - 播放时视口跟随播放头；手动滚动后暂停跟随，出现"回到播放头"按钮。
// - 底部总览条显示整段视频与当前视口：拖动视口平移，拖动两端改缩放，点击跳转。

const LABEL_W = 140;
const MIN_ZOOM = 1;
const MAX_ZOOM = 80;
const STEPS = [0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300];

const clamp = (v: number, a: number, b: number) => Math.min(b, Math.max(a, v));

interface Props {
  project: Project;
  time: number;
  /** 是否正在播放（播放中手动滚动会暂停跟随）。 */
  playing: boolean;
  selSeg: string | null;
  selTarget: string | null;
  onlyPending: boolean;
  onTogglePending: () => void;
  onSeek: (t: number) => void;
  onSelectSeg: (id: string) => void;
  onSelectTarget: (id: string) => void;
  onAccept: (id: string) => void;
  onReject: (id: string) => void;
}

export function Timeline({
  project,
  time,
  playing,
  selSeg,
  selTarget,
  onlyPending,
  onTogglePending,
  onSeek,
  onSelectSeg,
  onSelectTarget,
  onAccept,
  onReject,
}: Props) {
  const dur = project.video && project.video.duration > 0 ? project.video.duration : 1;
  const scrollRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const ovRef = useRef<HTMLDivElement>(null);

  const [width, setWidth] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [follow, setFollow] = useState(true);
  const [followPaused, setFollowPaused] = useState(false);
  const [scrollX, setScrollX] = useState(0);
  const [scrubbing, setScrubbing] = useState(false);
  const [hover, setHover] = useState<{ x: number; t: number } | null>(null);

  const pendingScroll = useRef<number | null>(null);
  const progUntil = useRef(0);
  const ovDrag = useRef<{ mode: 'pan' | 'l' | 'r'; x0: number; start0: number; end0: number } | null>(null);

  // 视口尺寸：可视轨道宽度 = 滚动容器宽度 - 标签列
  const visW = Math.max(1, width - LABEL_W);
  const fit = visW / dur;
  const pps = fit * zoom;
  const visibleSec = visW / pps;
  const viewStart = scrollX / pps;
  const playheadX = LABEL_W + time * pps - scrollX;
  const playheadVisible = playheadX >= LABEL_W && playheadX <= width - 8;

  // 测量滚动容器宽度
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => setWidth(el.clientWidth);
    update();
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(update) : null;
    ro?.observe(el);
    window.addEventListener('resize', update);
    return () => {
      ro?.disconnect();
      window.removeEventListener('resize', update);
    };
  }, []);

  // 缩放后需要改变滚动位置时，在 DOM 更新后应用
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && pendingScroll.current !== null) {
      setScroll(pendingScroll.current, el);
      pendingScroll.current = null;
    }
  });

  function setScroll(px: number, el = scrollRef.current) {
    if (!el) return;
    progUntil.current = performance.now() + 120;
    el.scrollLeft = clamp(px, 0, Math.max(0, el.scrollWidth - el.clientWidth));
    setScrollX(el.scrollLeft);
  }

  function zoomTo(z: number, anchorTime: number, anchorSx: number) {
    const nz = clamp(z, MIN_ZOOM, MAX_ZOOM);
    pendingScroll.current = LABEL_W + anchorTime * fit * nz - anchorSx;
    setZoom(nz);
  }

  function sxFor(t: number) {
    return clamp(LABEL_W + t * pps - scrollX, LABEL_W, Math.max(LABEL_W, width - 20));
  }

  function zoomBy(f: number) {
    zoomTo(zoom * f, time, sxFor(time));
  }

  function zoomFit() {
    pendingScroll.current = 0;
    setZoom(1);
  }

  function zoomToRange(t0: number, t1: number) {
    const nz = clamp(dur / Math.max(4, t1 - t0 + 2), MIN_ZOOM, MAX_ZOOM);
    pendingScroll.current = (t0 - 0.5) * fit * nz;
    setZoom(nz);
  }

  // 把时刻滚进可视区（用于下一待确认、点击段等）
  function reveal(t: number) {
    const x = LABEL_W + t * pps;
    const el = scrollRef.current;
    if (!el) return;
    const left = el.scrollLeft + LABEL_W;
    const right = el.scrollLeft + el.clientWidth - 16;
    if (x < left || x > right) setScroll(t * pps - visW * 0.1, el);
  }

  function centerOn(t: number) {
    setFollowPaused(false);
    setScroll(t * pps - visW / 2);
  }

  // 播放时跟随播放头（擦洗中不自动滚动）
  useEffect(() => {
    if (!follow || followPaused || scrubbing) return;
    const el = scrollRef.current;
    if (!el) return;
    const x = LABEL_W + time * pps;
    const left = el.scrollLeft + LABEL_W;
    const right = el.scrollLeft + el.clientWidth - 8;
    if (x < left || x > right) setScroll(x - LABEL_W - visW * 0.1, el);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [time, pps, follow, followPaused, scrubbing]);

  // Ctrl/⌘ + 滚轮缩放（需要非被动监听才能阻止页面缩放）
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      const r = el.getBoundingClientRect();
      const sx = e.clientX - r.left;
      const anchor = (el.scrollLeft + sx - LABEL_W) / pps;
      zoomTo(zoom * Math.exp(-e.deltaY * 0.0015), anchor, sx);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
    // zoomTo 闭包依赖 zoom / pps / fit
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [zoom, pps, fit]);

  // 刻度（只生成可视范围内的）
  const ticks = useMemo(() => {
    const step = STEPS.find((s) => s * pps >= 72) ?? 300;
    const minor = step / 5;
    const from = Math.max(0, Math.floor(viewStart / minor) * minor - minor);
    const to = Math.min(dur, viewStart + visibleSec + minor);
    const out: { s: number; major: boolean; label: string }[] = [];
    for (let s = from; s <= to + 1e-9; s += minor) {
      const major = Math.abs(s / step - Math.round(s / step)) < 1e-6;
      const m = Math.floor(s / 60);
      const sec = s - m * 60;
      const label = step < 1
        ? `${String(m).padStart(2, '0')}:${sec.toFixed(1).padStart(4, '0')}`
        : `${String(m).padStart(2, '0')}:${String(Math.round(sec)).padStart(2, '0')}`;
      out.push({ s, major, label });
    }
    return out;
  }, [pps, viewStart, visibleSec, dur]);

  // 指针：擦洗、点击段、点击标签
  function scrubTo(clientX: number) {
    const r = wrapRef.current?.getBoundingClientRect();
    if (!r) return;
    onSeek(clamp((clientX - r.left - LABEL_W) / pps, 0, dur));
  }

  function onPointerDown(e: ReactPointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    const blk = target.closest<HTMLElement>('.tl-blk');
    if (blk?.dataset.seg) {
      const seg = project.segments.find((s) => s.id === blk.dataset.seg);
      onSelectSeg(blk.dataset.seg);
      if (seg) onSeek(seg.t0);
      return;
    }
    const lab = target.closest<HTMLElement>('.tl-lab');
    if (lab) {
      if (lab.dataset.target) onSelectTarget(lab.dataset.target);
      return;
    }
    if (target.closest('.tl-ruler, .tl-lane, .tl-knob')) {
      scrollRef.current?.setPointerCapture(e.pointerId);
      setScrubbing(true);
      scrubTo(e.clientX);
      e.preventDefault();
    }
  }

  function onPointerMove(e: ReactPointerEvent<HTMLDivElement>) {
    if (scrubbing) {
      scrubTo(e.clientX);
      return;
    }
    const r = wrapRef.current?.getBoundingClientRect();
    if (!r) return;
    const x = e.clientX - r.left;
    const target = e.target as HTMLElement;
    if (x >= LABEL_W && target.closest('.tl-ruler, .tl-lane')) {
      setHover({ x, t: clamp((x - LABEL_W) / pps, 0, dur) });
    } else {
      setHover(null);
    }
  }

  function onScroll(e: UIEvent<HTMLDivElement>) {
    const el = e.currentTarget;
    setScrollX(el.scrollLeft);
    // 播放中且不是程序滚动时，视为用户手动滚动，暂停跟随
    if (performance.now() > progUntil.current && follow && playing) setFollowPaused(true);
  }

  // 总览条
  function onOvDown(e: ReactPointerEvent<HTMLDivElement>) {
    const el = ovRef.current;
    if (!el || e.button !== 0) return;
    const target = e.target as HTMLElement;
    const r = el.getBoundingClientRect();
    el.setPointerCapture(e.pointerId);
    const start = viewStart;
    const end = viewStart + visibleSec;
    const edge = target.dataset.edge;
    if (edge === 'l' || edge === 'r') {
      ovDrag.current = { mode: edge, x0: e.clientX, start0: start, end0: end };
    } else if (target.classList.contains('tl-vp')) {
      ovDrag.current = { mode: 'pan', x0: e.clientX, start0: start, end0: end };
    } else {
      centerOn(clamp(((e.clientX - r.left) / r.width) * dur, 0, dur));
      ovDrag.current = null;
    }
  }

  function onOvMove(e: ReactPointerEvent<HTMLDivElement>) {
    const d = ovDrag.current;
    const el = ovRef.current;
    if (!d || !el) return;
    const r = el.getBoundingClientRect();
    if (d.mode === 'pan') {
      const dt = ((e.clientX - d.x0) / r.width) * dur;
      setFollowPaused(true);
      setScroll((d.start0 + dt) * pps);
      return;
    }
    const sec = clamp(((e.clientX - r.left) / r.width) * dur, 0, dur);
    if (d.mode === 'r') {
      const end = Math.max(sec, d.start0 + 2);
      const nz = clamp(dur / (end - d.start0), MIN_ZOOM, MAX_ZOOM);
      pendingScroll.current = d.start0 * fit * nz;
      setZoom(nz);
    } else {
      const start = Math.min(sec, d.end0 - 2);
      const nz = clamp(dur / (d.end0 - start), MIN_ZOOM, MAX_ZOOM);
      pendingScroll.current = start * fit * nz;
      setZoom(nz);
    }
  }

  function onOvUp() {
    ovDrag.current = null;
  }

  // 数据
  const pendingCount = project.segments.filter((s) => s.status === 'pending' || s.status === 'low').length;
  const sel = project.segments.find((s) => s.id === selSeg);
  const next = nextPending(project, time);
  const ovSegs = useMemo(
    () =>
      project.segments
        .filter((s) => s.status !== 'rejected')
        .map((s) => ({ id: s.id, left: (s.t0 / dur) * 100, width: Math.max(0.3, ((s.t1 - s.t0) / dur) * 100), color: project.targets.find((t) => t.id === s.target)?.color ?? '#888' })),
    [project, dur],
  );

  const laneWidth = dur * pps;
  const wrapWidth = LABEL_W + laneWidth;

  return (
    <section className="timeline">
      <div className="tl-head">
        <b>时间轴</b>
        <span className="muted small">播放头 {formatTime(time)}</span>
        <span className="sp" />
        <div className="tl-zoom" title="缩放：Ctrl+滚轮 或 按钮">
          <button className="ghost" onClick={() => zoomBy(1 / 1.5)} disabled={zoom <= MIN_ZOOM + 1e-6} title="缩小">−</button>
          <span className="zl">{zoom <= 1.001 ? `全片 · ${Math.round(dur)} 秒可见` : `×${zoom.toFixed(1)} · ${visibleSec.toFixed(0)} 秒可见`}</span>
          <button className="ghost" onClick={() => zoomBy(1.5)} disabled={zoom >= MAX_ZOOM - 1e-6} title="放大">＋</button>
          <button onClick={zoomFit} title="适应全部">适应</button>
        </div>
        <label className="toggle" title="播放时视口跟随播放头；手动滚动后暂停">
          <input type="checkbox" checked={follow} onChange={(e) => { setFollow(e.target.checked); setFollowPaused(false); }} /> 跟随播放
        </label>
        <button className={onlyPending ? 'on' : ''} onClick={onTogglePending}>只看待确认（{pendingCount}）</button>
        <button
          disabled={!next}
          title="下一待确认 (F)"
          onClick={() => {
            if (!next) return;
            onSelectSeg(next.id);
            onSeek(next.t0);
            reveal(next.t0);
          }}
        >
          下一待确认 <kbd>F</kbd>
        </button>
      </div>

      <div className="tl-sel">
        {sel ? (
          <>
            <span className="dot" style={{ background: project.targets.find((t) => t.id === sel.target)?.color }} />
            <b>{project.targets.find((t) => t.id === sel.target)?.name ?? '目标'}</b>
            <span>{formatTime(sel.t0)} – {formatTime(sel.t1)}（{(sel.t1 - sel.t0).toFixed(1)} s）</span>
            <span className={`badge ${sel.status}`}>{({ confirmed: '已确认', pending: '待确认', low: '低置信', rejected: '已否决' } as const)[sel.status]}</span>
            <span className="muted">置信 {sel.conf.toFixed(2)}</span>
            <span className="sp" />
            <button className="primary" disabled={sel.status === 'confirmed'} onClick={() => onAccept(sel.id)}>接受 <kbd>Enter</kbd></button>
            <button disabled={sel.status === 'rejected'} onClick={() => onReject(sel.id)}>否决 <kbd>Del</kbd></button>
            <button onClick={() => zoomToRange(sel.t0, sel.t1)}>放大到该段</button>
          </>
        ) : (
          <span className="muted">单击段可选中（双击放大到该段）。选中后可在这里接受或否决。</span>
        )}
      </div>

      <div className="tl-scroll" ref={scrollRef} onScroll={onScroll} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={() => setScrubbing(false)} onPointerLeave={() => { if (!scrubbing) setHover(null); }}>
        <div className="tl-wrap" ref={wrapRef} style={{ width: wrapWidth } as CSSProperties}>
          <div className="tl-row tl-ruler-row">
            <div className="tl-lab">时间（秒）</div>
            <div className="tl-trk tl-ruler" style={{ width: laneWidth }}>
              {ticks.map((t) => (
                <div key={t.s} className={`tl-tick${t.major ? '' : ' minor'}`} style={{ left: t.s * pps }}>
                  {t.major ? t.label : null}
                </div>
              ))}
            </div>
          </div>

          {project.targets.map((tg) => {
            const segs = project.segments.filter(
              (s) => s.target === tg.id && s.status !== 'rejected' && (!onlyPending || s.status === 'pending' || s.status === 'low'),
            );
            return (
              <div className="tl-row" key={tg.id}>
                <div className={`tl-lab ${selTarget === tg.id ? 'sel' : ''} ${tg.enabled ? '' : 'off'}`} data-target={tg.id}>
                  <span className="dot" style={{ background: tg.color }} />
                  {tg.name}
                  <small>{!tg.enabled ? '已停用' : `${project.segments.filter((s) => s.target === tg.id && s.status !== 'rejected').length} 段`}</small>
                </div>
                <div className="tl-trk tl-lane" style={{ width: laneWidth }}>
                  {segs.map((s) => {
                    const w = (s.t1 - s.t0) * pps;
                    return (
                      <div
                        key={s.id}
                        className={`tl-blk st-${s.status} ${sel?.id === s.id ? 'sel' : ''}`}
                        data-seg={s.id}
                        style={{ left: s.t0 * pps, width: Math.max(4, w), '--c': tg.color } as CSSProperties}
                        title={`${tg.name} ${formatTime(s.t0)} – ${formatTime(s.t1)}（双击放大）`}
                        onDoubleClick={() => zoomToRange(s.t0, s.t1)}
                      >
                        {w > 60 ? `${s.t0.toFixed(0)}s` : ''}
                      </div>
                    );
                  })}
                </div>
              </div>
            );
          })}

          <div className="tl-ph" style={{ left: LABEL_W + time * pps }}>
            <div className="tl-knob" />
          </div>
          {hover && (
            <>
              <div className="tl-guide" style={{ left: hover.x }} />
              <div className="tl-tip" style={{ left: hover.x }}>{formatTime(hover.t)}</div>
            </>
          )}
        </div>
      </div>

      {!playheadVisible && (
        <button className="tl-back" onClick={() => centerOn(time)} title="把播放头放回可视区域">
          回到播放头 ⤴
        </button>
      )}

      <div className="tl-ov-row">
        <div className="tl-ov-sp">整段总览</div>
        <div className="tl-ov" ref={ovRef} onPointerDown={onOvDown} onPointerMove={onOvMove} onPointerUp={onOvUp} onPointerCancel={onOvUp}>
          {ovSegs.map((s) => (
            <div key={s.id} className="tl-ovseg" style={{ left: `${s.left}%`, width: `${s.width}%`, background: s.color }} />
          ))}
          <div className="tl-vp" style={{ left: `${(viewStart / dur) * 100}%`, width: `${Math.max(1.5, (visibleSec / dur) * 100)}%` }}>
            <i className="tl-edge l" data-edge="l" />
            <i className="tl-edge r" data-edge="r" />
          </div>
        </div>
      </div>
    </section>
  );
}
