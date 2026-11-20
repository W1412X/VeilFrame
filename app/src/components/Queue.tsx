export type JobState = 'queued' | 'scanning' | 'review' | 'done' | 'failed';

export interface Job {
  id: string;
  file: string;
  duration: string;
  state: JobState;
  progress: number;
  found: number;
  pending: number;
  eta?: string;
}

export const SAMPLE_JOBS: Job[] = [
  { id: 'j1', file: '会议录像_0412.mp4', duration: '45:20', state: 'scanning', progress: 62, found: 41, pending: 7, eta: '3:10' },
  { id: 'j2', file: '产品演示_final.mov', duration: '03:45', state: 'review', progress: 100, found: 18, pending: 4 },
  { id: 'j3', file: '街拍_素材07.mp4', duration: '12:08', state: 'queued', progress: 0, found: 0, pending: 0 },
  { id: 'j4', file: '监控_夜班.mkv', duration: '2:10:00', state: 'done', progress: 100, found: 96, pending: 0 },
];

const STATE_LABEL: Record<JobState, string> = {
  queued: '排队',
  scanning: '扫描中',
  review: '待复核',
  done: '已完成',
  failed: '失败',
};

interface Props {
  jobs: Job[];
  onAction: (job: Job) => void;
  onAdd: () => void;
}

export function Queue({ jobs, onAction, onAdd }: Props) {
  return (
    <div className="page">
      <div className="page-head">
        <h2>批量队列</h2>
        <span className="muted small">断点续扫 · 失败自动重试 · 夜间跑批</span>
        <span className="sp" />
        <button onClick={onAdd}>+ 添加视频</button>
      </div>
      <table className="tbl">
        <thead>
          <tr>
            <th>视频</th>
            <th>时长</th>
            <th>状态</th>
            <th>进度</th>
            <th>检出段</th>
            <th>待确认</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {jobs.map((j) => (
            <tr key={j.id}>
              <td>{j.file}</td>
              <td>{j.duration}</td>
              <td>
                <span className={`status ${j.state}`}>{STATE_LABEL[j.state]}</span>
              </td>
              <td>
                <div className="bar">
                  <i style={{ width: `${j.progress}%` }} />
                </div>
                <span className="muted small">
                  {j.progress}%{j.eta ? ` · ETA ${j.eta}` : ''}
                </span>
              </td>
              <td>{j.state === 'queued' ? '—' : j.found}</td>
              <td>{j.state === 'queued' ? '—' : j.pending}</td>
              <td>
                <button className="sm" onClick={() => onAction(j)}>
                  {actionLabel(j.state)}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function actionLabel(state: JobState): string {
  switch (state) {
    case 'queued':
      return '开始扫描';
    case 'scanning':
      return '取消';
    case 'review':
      return '进入复核';
    case 'done':
      return '导出';
    case 'failed':
      return '重试';
  }
}
