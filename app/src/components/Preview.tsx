import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import type { Project, Quad, Segment } from '../domain/types';
import { activeSegments, pickKeyframe } from '../domain/segments';
import { assetUrl } from '../api';

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

/** 画面预览：真实视频 + SVG 叠加。叠加层坐标即视频像素坐标。 */
export function Preview(props: Props) {
  const { project, time, playing, selTarget, selSeg, compare, roiTargetId, onTime, onPlayingChange, onSelectSeg, onEditQuad, onPickRoi } = props;
  const svgRef = useRef<SVGSVGElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [rect, setRect] = useState<RectDrag | null>(null);
  const [videoError, setVideoError] = useState<string | null>(null);
  const onTimeRef = useRef(onTime);
  onTimeRef.current = onTime;
  const video = project.video;
  const W = video?.width ?? 1920;
  const H = video?.height ?? 1080;
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
        <video
          ref={videoRef}
          className="video"
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
          const quad: Quad | undefined = drag && drag.segId === s.id ? drag.quad : pickKeyframe(s, time)?.quad;
          if (!quad) return null;
          const pts = quad.map((p) => `${p[0]},${p[1]}`).join(' ');
          const selected = selSeg === s.id;
          const dashed = s.status === 'pending' || s.status === 'low';
          const fill = effectFill(tg.style.kind, s.id, tg.style.strength, tg.color);
          return (
            <g
              key={s.id}
              className={`seg st-${s.status}`}
              onPointerDown={(e) => {
                e.stopPropagation();
                onSelectSeg(s.id);
              }}
            >
              {fill.defs}
              <polygon
                points={pts}
                fill={fill.paint}
                fillOpacity={s.status === 'low' ? 0.5 : 0.9}
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

/** 按打码类型生成叠加填充：马赛克为棋盘格，模糊为柔化渐变，色块为纯色。 */
function effectFill(kind: string, id: string, strength: number, color: string) {
  if (kind === 'mosaic') {
    const tile = Math.round(16 + strength * 24);
    const pid = `pat-${id}`;
    return {
      paint: `url(#${pid})`,
      defs: (
        <defs key={pid}>
          <pattern id={pid} width={tile} height={tile} patternUnits="userSpaceOnUse">
            <rect width={tile} height={tile} fill="#7d828c" />
            <rect width={tile / 2} height={tile / 2} fill="#c4c8cf" />
            <rect x={tile / 2} y={tile / 2} width={tile / 2} height={tile / 2} fill="#c4c8cf" />
          </pattern>
        </defs>
      ),
    };
  }
  if (kind === 'blur') {
    const pid = `blur-${id}`;
    return {
      paint: `url(#${pid})`,
      defs: (
        <defs key={pid}>
          <radialGradient id={pid}>
            <stop offset="0%" stopColor="#e8eaee" />
            <stop offset="100%" stopColor="#9aa0a8" />
          </radialGradient>
        </defs>
      ),
    };
  }
  return { paint: color, defs: null };
}
