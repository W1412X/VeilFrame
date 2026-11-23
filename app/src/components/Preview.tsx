import { useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import type { Project, Quad, Segment } from '../domain/types';
import { activeSegments } from '../domain/segments';
import { quadAt } from '../domain/render';
import { assetUrl } from '../api';
import { EffectRenderer, type RenderItem } from './effectRenderer';

interface Props {
  project: Project;
  time: number;
  playing: boolean;
  selTarget: string | null;
  selSeg: string | null;
  compare: boolean;
  /** 处于框选模式的区域目标；非空时在画面上拖拽会回调 `onPickRoi`。 */
  roiTargetId: string | null;
  onTime: (t: number) => void;
  onPlayingChange: (playing: boolean) => void;
  onSelectSeg: (id: string | null) => void;
  /** 拖动四角结束时调用：在当前时刻写入关键帧。 */
  onEditQuad: (segId: string, t: number, quad: Quad) => void;
  onPickRoi: (roi: [number, number, number, number]) => void;
}

interface Drag {
  segId: string;
  index: number;
  quad: Quad;
}

interface RectDrag {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/**
 * 当前时刻要打码的区域。四边形与运动速度按导出的同一套规则计算；正在拖动的段使用拖动中的四边形。
 */
function renderItems(project: Project, t: number, fps: number, drag: Drag | null): RenderItem[] {
  const items: RenderItem[] = [];
  for (const seg of activeSegments(project, t)) {
    const target = project.targets.find((x) => x.id === seg.target);
    if (!target) continue;
    const dragging = drag !== null && drag.segId === seg.id;
    const sampled = quadAt(seg, t, fps);
    items.push({
      style: target.style,
      quad: dragging && drag ? drag.quad : sampled.quad,
      speed: dragging ? 0 : sampled.speed,
    });
  }
  return items;
}

/**
 * 画面预览。打码在 canvas 上逐帧实时计算（真实画面像素），SVG 叠加层只负责轮廓、标签与拖拽手柄。
 * 叠加层与 canvas 的坐标都是视频像素坐标。
 */
export function Preview(props: Props) {
  const { project, time, playing, selTarget, selSeg, compare, roiTargetId, onTime, onPlayingChange, onSelectSeg, onEditQuad, onPickRoi } = props;
  const svgRef = useRef<SVGSVGElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [rect, setRect] = useState<RectDrag | null>(null);
  const [videoError, setVideoError] = useState<string | null>(null);
  const onTimeRef = useRef(onTime);
  onTimeRef.current = onTime;
  const video = project.video;
  const W = video?.width ?? 1920;
  const H = video?.height ?? 1080;
  const fps = video?.fps && video.fps > 0 ? video.fps : 30;
  const renderer = useMemo(() => new EffectRenderer(W, H), [W, H]);
  const shown: Segment[] = compare ? [] : activeSegments(project, time);
  const activeTarget = project.targets.find((t) => t.id === selTarget);
  const roiShown = !compare && (roiTargetId ? null : activeTarget?.roi ?? null);

  useEffect(() => {
    setVideoError(null);
  }, [video?.path]);

  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    if (playing) void v.play().catch(() => onPlayingChange(false));
    else v.pause();
  }, [playing, onPlayingChange]);

  useEffect(() => {
    const v = videoRef.current;
    if (!v || !Number.isFinite(time)) return;
    if (Math.abs(v.currentTime - time) > 0.15) v.currentTime = time;
  }, [time]);

  // timeupdate 只有约 4 Hz，播放时用 rAF 让播放头更顺滑。
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const v = videoRef.current;
      if (v) onTimeRef.current(v.currentTime);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing]);

  // 实时打码循环：每个动画帧检查视频时刻是否变化；变化（播放、拖动进度条、编辑打码参数）时重绘。
  // 暂停且时刻未变时不重复计算，避免空耗 CPU。
  useEffect(() => {
    let raf = 0;
    let lastKey = '';
    const frame = () => {
      raf = requestAnimationFrame(frame);
      const v = videoRef.current;
      const c = canvasRef.current;
      if (!v || !c || v.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) return;
      const key = String(v.currentTime);
      if (key === lastKey) return;
      lastKey = key;
      const ctx = c.getContext('2d');
      if (!ctx) return;
      const items = compare ? [] : renderItems(project, v.currentTime, fps, drag);
      renderer.render(ctx, v, items);
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [renderer, project, compare, drag, fps]);

  const toVideo = (clientX: number, clientY: number): [number, number] => {
    const svg = svgRef.current!;
    const pt = svg.createSVGPoint();
    pt.x = clientX;
    pt.y = clientY;
    const p = pt.matrixTransform(svg.getScreenCTM()!.inverse());
    return [Math.max(0, Math.min(W, p.x)), Math.max(0, Math.min(H, p.y))];
  };

  const startDrag = (e: ReactPointerEvent<SVGCircleElement>, seg: Segment, index: number, quad: Quad) => {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ segId: seg.id, index, quad: quad.map((p) => [p[0], p[1]]) as Quad });
  };

  const moveDrag = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (drag) {
      const [x, y] = toVideo(e.clientX, e.clientY);
      const quad = drag.quad.map((p) => [p[0], p[1]]) as Quad;
      quad[drag.index] = [x, y];
      setDrag({ ...drag, quad });
    } else if (rect) {
      const [x, y] = toVideo(e.clientX, e.clientY);
      setRect({ ...rect, x1: x, y1: y });
    }
  };

  const finishPointer = () => {
    if (rect) {
      const x = Math.min(rect.x0, rect.x1);
      const y = Math.min(rect.y0, rect.y1);
      const w = Math.abs(rect.x1 - rect.x0);
      const h = Math.abs(rect.y1 - rect.y0);
      setRect(null);
      if (w >= 8 && h >= 8) onPickRoi([Math.round(x), Math.round(y), Math.round(w), Math.round(h)]);
      return;
    }
    if (drag) {
      onEditQuad(drag.segId, time, drag.quad);
      setDrag(null);
    }
  };

  const unit = Math.max(W, H) / 1080;
  const stroke = 3 * unit;
  const handle = 12 * unit;
  const font = 26 * unit;

  return (
    <div
      className="frame"
      style={{ aspectRatio: `${W} / ${H}`, width: `min(100%, calc((100vh - 340px) * ${W} / ${H}))` }}
    >
      {video ? (
        <>
          {/* 视频元素只负责解码与播放；可见画面由下面的 canvas 实时绘制（含打码）。 */}
          <video
            ref={videoRef}
            className="video-source"
            src={assetUrl(video.path)}
            playsInline
            preload="auto"
            onTimeUpdate={(e) => onTime(e.currentTarget.currentTime)}
            onLoadedMetadata={(e) => {
              if (time > 0) e.currentTarget.currentTime = time;
            }}
            onEnded={() => onPlayingChange(false)}
            onError={() => setVideoError('无法解码该视频，请确认编码格式或 ffmpeg 可用。')}
          />
          <canvas ref={canvasRef} className="video" width={W} height={H} />
        </>
      ) : (
        <div className="stage-empty">
          <div>还没有打开视频</div>
          <small>从顶栏选择「打开视频」或「打开项目」</small>
        </div>
      )}

      {videoError && <div className="orig-badge">{videoError}</div>}

      <svg
        ref={svgRef}
        className="overlay"
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="xMidYMid meet"
        onPointerDown={(e) => {
          if (e.target !== e.currentTarget) return;
          if (roiTargetId) {
            const [x, y] = toVideo(e.clientX, e.clientY);
            e.currentTarget.setPointerCapture(e.pointerId);
            setRect({ x0: x, y0: y, x1: x, y1: y });
            return;
          }
          onSelectSeg(null);
        }}
        onPointerMove={moveDrag}
        onPointerUp={finishPointer}
        onPointerCancel={finishPointer}
      >
        {roiShown && (
          <rect
            x={roiShown[0]}
            y={roiShown[1]}
            width={roiShown[2]}
            height={roiShown[3]}
            fill="none"
            stroke={activeTarget?.color ?? '#fff'}
            strokeWidth={stroke * 0.8}
            strokeDasharray={`${unit * 10} ${unit * 8}`}
            pointerEvents="none"
          />
        )}
        {rect && (
          <rect
            x={Math.min(rect.x0, rect.x1)}
            y={Math.min(rect.y0, rect.y1)}
            width={Math.abs(rect.x1 - rect.x0)}
            height={Math.abs(rect.y1 - rect.y0)}
            fill="rgba(47, 91, 211, 0.18)"
            stroke="#fff"
            strokeWidth={stroke}
            strokeDasharray={`${unit * 10} ${unit * 8}`}
            pointerEvents="none"
          />
        )}
        {shown.map((s) => {
          const tg = project.targets.find((x) => x.id === s.target);
          if (!tg) return null;
          const quad: Quad = drag && drag.segId === s.id ? drag.quad : quadAt(s, time, fps).quad;
          const pts = quad.map((p) => `${p[0]},${p[1]}`).join(' ');
          const selected = selSeg === s.id;
          const dashed = s.status === 'pending' || s.status === 'low';
          return (
            <g
              key={s.id}
              className={`seg st-${s.status}`}
              onPointerDown={(e) => {
                e.stopPropagation();
                onSelectSeg(s.id);
              }}
            >
              {/* 透明填充只用于命中测试，真正的打码效果由 canvas 绘制 */}
              <polygon
                points={pts}
                fill="transparent"
                stroke={tg.color}
                strokeWidth={selected ? stroke * 2 : stroke}
                strokeDasharray={dashed ? `${unit * 16} ${unit * 10}` : undefined}
              />
              <text x={quad[0][0]} y={quad[0][1] - 10 * unit} fill={tg.color} fontSize={font} fontWeight={600}>
                {tg.name} {s.conf.toFixed(2)}
              </text>
              {selected &&
                quad.map((p, i) => (
                  <circle
                    key={i}
                    cx={p[0]}
                    cy={p[1]}
                    r={handle}
                    fill="#fff"
                    stroke={tg.color}
                    strokeWidth={stroke}
                    style={{ cursor: 'move' }}
                    onPointerDown={(e) => startDrag(e, s, i, quad)}
                  />
                ))}
            </g>
          );
        })}
      </svg>
    </div>
  );
}
