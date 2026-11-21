import type { CSSProperties, PointerEvent } from 'react';
import type { Project } from '../domain/types';
import { nextPending } from '../domain/segments';

interface Props {
  project: Project;
  time: number;
  selSeg: string | null;
  selTarget: string | null;
  onlyPending: boolean;
  onTogglePending: () => void;
  onSeek: (t: number) => void;
  onSelectSeg: (id: string) => void;
  onSelectTarget: (id: string) => void;
}

const LABEL_W = 130;

export function Timeline({
  project,
  time,
  selSeg,
  selTarget,
  onlyPending,
  onTogglePending,
  onSeek,
  onSelectSeg,
  onSelectTarget,
}: Props) {
  const dur = project.video?.duration ?? 1;
  const pct = (t: number) => (t / dur) * 100;
  const pendingCount = project.segments.filter((s) => s.status === 'pending' || s.status === 'low').length;
  const hasNext = nextPending(project, time) !== undefined;

  const seekFromTrack = (e: PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    onSeek(((e.clientX - r.left) / r.width) * dur);
  };

  return (
    <section className="timeline">
      <div className="tl-top">
        <b>时间轴</b>
        <span className="muted small">播放头 {time.toFixed(1)}s</span>
        <span className="sp" />
        <button className={onlyPending ? 'on' : ''} onClick={onTogglePending}>
          只看待确认（{pendingCount}）
        </button>
        <button disabled={!hasNext} title="F" onClick={() => {
          const n = nextPending(project, time);
          if (n) onSeek(n.t0);
        }}>
          下一待确认
        </button>
      </div>
      <div className="tl-body">
        {project.targets.map((t) => {
          const segs = project.segments.filter(
            (s) => s.target === t.id && s.status !== 'rejected' && (!onlyPending || s.status === 'pending' || s.status === 'low'),
          );
          return (
            <div className="row" key={t.id}>
              <div
                className={`lab ${selTarget === t.id ? 'sel' : ''}`}
                style={{ opacity: t.enabled ? 1 : 0.4 }}
                onClick={() => onSelectTarget(t.id)}
              >
                <span className="dot" style={{ background: t.color }} />
                {t.name}
              </div>
              <div className="trk" onPointerDown={seekFromTrack}>
                {segs.map((s) => (
                  <div
                    key={s.id}
                    className={`blk st-${s.status} ${selSeg === s.id ? 'sel' : ''}`}
                    style={
                      {
                        left: `${pct(s.t0)}%`,
                        width: `${Math.max(0.4, pct(s.t1) - pct(s.t0))}%`,
                        '--c': t.color,
                      } as CSSProperties
                    }
                    title={`${t.name} ${s.t0.toFixed(1)}–${s.t1.toFixed(1)}s`}
                    onPointerDown={(e) => {
                      e.stopPropagation();
                      onSelectSeg(s.id);
                      onSeek(s.t0);
                    }}
                  />
                ))}
              </div>
            </div>
          );
        })}
        <div className="playhead" style={{ left: `calc(${LABEL_W}px + (100% - ${LABEL_W}px) * ${time / dur})` }} />
      </div>
    </section>
  );
}
