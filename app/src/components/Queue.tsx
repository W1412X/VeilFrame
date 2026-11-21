import type { BatchJob, BatchState } from '../api';
import { formatTime } from '../domain/segments';

const STATE_LABEL: Record<BatchState, string> = {
  queued: '排队',
  scanning: '扫描中',
  review: '待复核',
  exporting: '导出中',
  done: '已完成',
  cancelled: '已取消',
  failed: '失败',
};

interface Props {
  jobs: BatchJob[];
  onAdd: () => void;
  onStart: () => void;
  onOpen: (job: BatchJob) => void;
  onCancel: (job: BatchJob) => void;
  onResume: (job: BatchJob) => void;
  onRemove: (job: BatchJob) => void;
}

/** 批量队列：目标取自当前项目，扫描结果保存为视频同目录的 .maskproj。 */
export function Queue({ jobs, onAdd, onStart, onOpen, onCancel, onResume, onRemove }: Props) {
  const queued = jobs.filter((j) => j.state === 'queued').length;
  const scanning = jobs.find((j) => j.state === 'scanning');
  const review = jobs.filter((j) => j.state === 'review').length;

  return (
    <div className="page">
      <div className="page-head">
        <h2>批量队列</h2>
        <span className="muted small">队列任务共用当前项目的目标 · 失败自动重试一次</span>
        <span className="sp" />
        {scanning && <span className="muted small">正在扫描：{scanning.file}</span>}
        {review > 0 && <span className="muted small">{review} 个待复核</span>}
        <button onClick={onAdd}>+ 添加视频</button>
        <button className="primary" disabled={queued === 0} onClick={onStart}>
          开始全部
        </button>
      </div>

      {jobs.length === 0 ? (
        <p className="muted">
          队列为空。先在编辑器里配置好目标，再添加要批量处理的视频；扫描完成后可从队列进入复核。
        </p>
      ) : (
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
                <td title={j.source}>{j.file}</td>
                <td>{formatTime(j.duration)}</td>
                <td>
                  <span className={`status ${j.state}`}>{STATE_LABEL[j.state]}</span>
                  {j.state === 'failed' && j.error && <div className="muted small">{j.error}</div>}
                </td>
                <td>
                  <div className="bar">
                    <i style={{ width: `${percent(j.progress, j.duration)}%` }} />
                  </div>
                  <span className="muted small">
                    {Math.round(percent(j.progress, j.duration))}%
                    {j.state === 'scanning' ? ` · ${formatTime(j.progress)}` : ''}
                  </span>
                </td>
                <td>{j.state === 'queued' || j.state === 'scanning' ? '—' : j.found}</td>
                <td>{j.state === 'queued' || j.state === 'scanning' ? '—' : j.pending}</td>
                <td>
                  <div className="btns" style={{ margin: 0 }}>
                    <button
                      className="sm"
                      disabled={j.state === 'exporting'}
                      onClick={() => primaryAction(j, onOpen, onCancel, onResume)}
                    >
                      {actionLabel(j.state)}
                    </button>
                    {j.state !== 'scanning' && (
                      <button className="ghost sm" title="从队列移除" onClick={() => onRemove(j)}>
                        移除
                      </button>
                    )}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function primaryAction(
  job: BatchJob,
  open: (j: BatchJob) => void,
  cancel: (j: BatchJob) => void,
  resume: (j: BatchJob) => void,
) {
  switch (job.state) {
    case 'queued':
    case 'scanning':
      cancel(job);
      return;
    case 'review':
    case 'done':
      open(job);
      return;
    case 'cancelled':
    case 'failed':
      resume(job);
      return;
    default:
      return;
  }
}

function actionLabel(state: BatchState): string {
  switch (state) {
    case 'queued':
      return '取消';
    case 'scanning':
      return '停止';
    case 'review':
      return '进入复核';
    case 'done':
      return '打开项目';
    case 'cancelled':
      return '重新扫描';
    case 'failed':
      return '重试';
    case 'exporting':
      return '导出中';
  }
}

function percent(progress: number, duration: number): number {
  if (duration <= 0) return 0;
  return Math.min(100, (progress / duration) * 100);
}
