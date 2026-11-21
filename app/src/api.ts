// 与 Tauri 后端通信的薄封装。浏览器中直接打开开发服务器时没有后端，
// 调用会抛出 NotDesktopError，界面据此给出提示。
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Project, Quad, Target } from './domain/types';

export class NotDesktopError extends Error {
  constructor() {
    super('需要在桌面应用中使用（当前是浏览器预览）');
  }
}

/** 是否运行在 Tauri 窗口里。 */
export const isDesktop = (): boolean => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

function requireDesktop(): void {
  if (!isDesktop()) throw new NotDesktopError();
}

/** 把本地视频路径转成 webview 可播放的 asset URL。 */
export function assetUrl(path: string): string {
  return convertFileSrc(path);
}

export interface ToolStatus {
  ffmpeg: string | null;
  ffprobe: string | null;
  ort: boolean;
  ocr_models: boolean;
}

/** 后端返回的完整状态快照（Rust 字段为 snake_case）。 */
export interface Snapshot {
  project: Project;
  project_path: string | null;
  can_undo: boolean;
  can_redo: boolean;
  autosave_error: string | null;
  scanning: boolean;
  exporting: boolean;
}

export interface ScanProgress {
  target_index: number;
  target_total: number;
  target_id: string;
  t: number;
  duration: number;
}

export interface ScanSummary {
  cancelled: boolean;
  scanned_until: number;
  segments: number;
  cache_warning: string | null;
}

export interface ScanDone {
  snapshot: Snapshot;
  summary: ScanSummary;
}

export interface ExportProgress {
  done: number;
  total: number;
}

export interface ExportDone {
  frames: number;
  masked_regions: number;
  output: string;
}

export type BatchState = 'queued' | 'scanning' | 'review' | 'exporting' | 'done' | 'cancelled' | 'failed';

/** 批量队列中的一条任务。项目文件保存在视频同目录。 */
export interface BatchJob {
  id: string;
  file: string;
  source: string;
  project_path: string;
  state: BatchState;
  progress: number;
  duration: number;
  found: number;
  pending: number;
  attempts: number;
  error: string | null;
}

export async function toolStatus(): Promise<ToolStatus> {
  requireDesktop();
  return invoke<ToolStatus>('tool_status');
}

export async function getSnapshot(): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('snapshot');
}

export async function openVideo(path: string): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('open_video', { path });
}

export async function openProject(path: string): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('open_project', { path });
}

/** 保存项目；不传路径时写回已有关联的路径。 */
export async function saveProject(path?: string | null): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('save_project', { path: path ?? null });
}

export async function undo(): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('undo');
}

export async function redo(): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('redo');
}

export async function addTarget(target: Target): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('add_target', { target });
}

export async function updateTarget(target: Target): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('update_target', { target });
}

export async function acceptSegment(segmentId: string): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('accept_segment', { segmentId });
}

export async function rejectSegment(segmentId: string, negative: boolean): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('reject_segment', { segmentId, negative });
}

export async function deleteSegment(segmentId: string): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('delete_segment', { segmentId });
}

export async function extendTrack(segmentId: string): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('extend_track', { segmentId });
}

export async function setKeyframe(segmentId: string, t: number, quad: Quad): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('set_keyframe', { segmentId, t, quad });
}

export async function addManualSegment(
  id: string,
  targetId: string,
  t0: number,
  t1: number,
  quad: Quad,
): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('add_manual_segment', { id, targetId, t0, t1, quad });
}

export async function scanTargets(targetIds: string[], interval = 0.5): Promise<ScanDone> {
  requireDesktop();
  return invoke<ScanDone>('scan_targets', { targetIds, interval });
}

export async function cancelScan(): Promise<void> {
  requireDesktop();
  await invoke('cancel_scan');
}

/** `queueJob` 非空时，导出成功会把对应批量任务标记为完成。 */
export async function exportVideo(
  output: string,
  codec: string,
  crf: number,
  includePending: boolean,
  queueJob: string | null,
): Promise<ExportDone> {
  requireDesktop();
  return invoke<ExportDone>('export_video', { output, codec, crf, includePending, queueJob });
}

export async function cancelExport(): Promise<void> {
  requireDesktop();
  await invoke('cancel_export');
}

export async function queueList(): Promise<BatchJob[]> {
  requireDesktop();
  return invoke<BatchJob[]>('queue_list');
}

/** 把视频加入队列；目标取自当前项目。返回实际新增数量（重复项跳过）。 */
export async function queueAdd(paths: string[]): Promise<number> {
  requireDesktop();
  return invoke<number>('queue_add', { paths });
}

export async function queueStart(): Promise<BatchJob[]> {
  requireDesktop();
  return invoke<BatchJob[]>('queue_start');
}

export async function queueCancel(jobId: string): Promise<void> {
  requireDesktop();
  await invoke('queue_cancel', { jobId });
}

export async function queueResume(jobId: string): Promise<void> {
  requireDesktop();
  await invoke('queue_resume', { jobId });
}

export async function queueRemove(jobId: string): Promise<void> {
  requireDesktop();
  await invoke('queue_remove', { jobId });
}

/** 载入某个已完成扫描的任务，回到编辑器复核。 */
export async function queueOpen(jobId: string): Promise<Snapshot> {
  requireDesktop();
  return invoke<Snapshot>('queue_open', { jobId });
}

export function onQueueChanged(cb: (jobs: BatchJob[]) => void): Promise<UnlistenFn> {
  return listen<BatchJob[]>('vf-queue-changed', (e) => cb(e.payload));
}

export function onScanProgress(cb: (p: ScanProgress) => void): Promise<UnlistenFn> {
  return listen<ScanProgress>('vf-scan-progress', (e) => cb(e.payload));
}

export function onExportProgress(cb: (p: ExportProgress) => void): Promise<UnlistenFn> {
  return listen<ExportProgress>('vf-export-progress', (e) => cb(e.payload));
}

/** 弹出文件选择框选择视频；取消时返回 null。 */
export async function pickVideo(): Promise<string | null> {
  requireDesktop();
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    multiple: false,
    filters: [{ name: '视频', extensions: ['mp4', 'mov', 'mkv', 'avi', 'm4v', 'webm'] }],
  });
  return typeof selected === 'string' ? selected : null;
}

/** 多选视频（批量队列）；取消时返回空数组。 */
export async function pickVideos(): Promise<string[]> {
  requireDesktop();
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    multiple: true,
    filters: [{ name: '视频', extensions: ['mp4', 'mov', 'mkv', 'avi', 'm4v', 'webm'] }],
  });
  if (Array.isArray(selected)) return selected;
  return typeof selected === 'string' ? [selected] : [];
}

/** 弹出文件选择框选择项目文件；取消时返回 null。 */
export async function pickProject(): Promise<string | null> {
  requireDesktop();
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    multiple: false,
    filters: [{ name: 'VeilFrame 项目', extensions: ['maskproj'] }],
  });
  return typeof selected === 'string' ? selected : null;
}

/** 弹出保存对话框选择项目路径；取消时返回 null。 */
export async function pickSavePath(defaultName: string): Promise<string | null> {
  requireDesktop();
  const { save } = await import('@tauri-apps/plugin-dialog');
  const path = await save({
    defaultPath: defaultName,
    filters: [{ name: 'VeilFrame 项目', extensions: ['maskproj'] }],
  });
  return path ?? null;
}

/** 选择图像目标的模板图；取消时返回 null。 */
export async function pickTemplate(): Promise<string | null> {
  requireDesktop();
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    multiple: false,
    filters: [{ name: '模板图', extensions: ['png', 'jpg', 'jpeg', 'bmp', 'webp'] }],
  });
  return typeof selected === 'string' ? selected : null;
}

/** 选择导出视频的保存路径；取消时返回 null。 */
export async function pickExportPath(defaultName: string): Promise<string | null> {
  requireDesktop();
  const { save } = await import('@tauri-apps/plugin-dialog');
  const path = await save({
    defaultPath: defaultName,
    filters: [{ name: '视频', extensions: ['mp4', 'mkv', 'mov'] }],
  });
  return path ?? null;
}
