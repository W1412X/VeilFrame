// 与 Tauri 后端通信的薄封装。浏览器中直接打开开发服务器时没有后端，
// 调用会返回 null / 抛出 NotDesktopError，界面据此给出提示。
import { invoke } from '@tauri-apps/api/core';
import type { Project } from './domain/types';

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

export interface ToolStatus {
  ffmpeg: string | null;
  ffprobe: string | null;
}

export async function toolStatus(): Promise<ToolStatus> {
  requireDesktop();
  return invoke<ToolStatus>('tool_status');
}

/** 弹出文件选择框选择视频；取消时返回 null。 */
export async function pickVideo(): Promise<string | null> {
  requireDesktop();
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    multiple: false,
    filters: [{ name: '视频', extensions: ['mp4', 'mov', 'mkv', 'avi', 'm4v'] }],
  });
  return typeof selected === 'string' ? selected : null;
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

/** 弹出保存对话框；取消时返回 null。 */
export async function pickSavePath(defaultName: string): Promise<string | null> {
  requireDesktop();
  const { save } = await import('@tauri-apps/plugin-dialog');
  const path = await save({
    defaultPath: defaultName,
    filters: [{ name: 'VeilFrame 项目', extensions: ['maskproj'] }],
  });
  return path ?? null;
}

export async function newProject(videoPath: string): Promise<Project> {
  requireDesktop();
  return invoke<Project>('new_project', { videoPath });
}

export async function loadProject(path: string): Promise<Project> {
  requireDesktop();
  return invoke<Project>('load_project', { path });
}

export async function saveProject(path: string, project: Project): Promise<void> {
  requireDesktop();
  await invoke('save_project', { path, project });
}
