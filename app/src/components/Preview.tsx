import { useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import type { Project, Quad, Segment } from '../domain/types';
import { activeSegments } from '../domain/editor';

interface Props {
  project: Project;
  time: number;
  selSeg: string | null;
  compare: boolean;
  onSelectSeg: (id: string | null) => void;
  /** 拖动四角结束时调用：在当前时刻写入关键帧。 */
  onEditQuad: (segId: string, t: number, quad: Quad) => void;
}

interface Drag {
  segId: string;
  index: number;
  quad: Quad;
}

/** 画面预览：示例底图 + SVG 叠加。叠加层坐标即视频像素坐标。 */
export function Preview({ project, time, selSeg, compare, onSelectSeg, onEditQuad }: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const video = project.video;
  const W = video?.width ?? 1920;
  const H = video?.height ?? 1080;
  const shown: Segment[] = compare ? [] : activeSegments(project, time);

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
    if (!drag) return;
    const [x, y] = toVideo(e.clientX, e.clientY);
    const quad = drag.quad.map((p) => [p[0], p[1]]) as Quad;
    quad[drag.index] = [x, y];
    setDrag({ ...drag, quad });
  };

  const endDrag = () => {
    if (!drag) return;
    onEditQuad(drag.segId, time, drag.quad);
    setDrag(null);
  };

  return (
    <div className="frame">
      <div className="scene">
        <div className="shape" style={{ left: '8%', top: '12%', width: '24%', height: '14%' }} />
        <div className="shape" style={{ left: '80%', top: '5%', width: '12%', height: '12%', borderRadius: 4 }} />
        <div className="shape" style={{ left: '30%', top: '62%', width: '16%', height: '9%' }} />
        <div className="shape" style={{ left: '52%', top: '68%', width: '22%', height: '14%' }} />
      </div>
      <svg
        ref={svgRef}
        className="overlay"
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="xMidYMid meet"
        onPointerDown={(e) => {
          if (e.target === e.currentTarget) onSelectSeg(null);
        }}
        onPointerMove={moveDrag}
        onPointerUp={endDrag}
        onPointerLeave={endDrag}
      >
        {shown.map((s) => {
          const tg = project.targets.find((x) => x.id === s.target);
          if (!tg) return null;
          const color = tg.color;
          const quad: Quad = drag && drag.segId === s.id ? drag.quad : pickKeyframe(s, time).quad;
          const pts = quad.map((p) => `${p[0]},${p[1]}`).join(' ');
          const selected = selSeg === s.id;
          const dashed = s.status === 'pending' || s.status === 'low';
          const fill = effectFill(tg.style.kind, s.id, tg.style.strength, color);
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
                stroke={color}
                strokeWidth={selected ? 6 : 3}
                strokeDasharray={dashed ? '16 10' : undefined}
              />
              <text x={quad[0][0]} y={quad[0][1] - 10} fill={color} fontSize={26} fontWeight={600}>
                {tg.name} {s.conf.toFixed(2)}
              </text>
              {selected &&
                quad.map((p, i) => (
                  <circle
                    key={i}
                    cx={p[0]}
                    cy={p[1]}
                    r={12}
                    fill="#fff"
                    stroke={color}
                    strokeWidth={4}
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

/** 按打码类型生成叠加填充：马赛克为棋盘格，模糊为柔化点阵，色块为纯色。 */
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

/** 取时刻 t 最近的关键帧四边形。 */
export function pickKeyframe(s: Segment, t: number) {
  const ks = s.keyframes;
  let best = ks[0];
  for (const k of ks) {
    if (Math.abs(k.t - t) < Math.abs(best.t - t)) best = k;
  }
  return best;
}
