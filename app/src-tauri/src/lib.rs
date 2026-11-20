//! VeilFrame 桌面外壳。把项目读写、视频探测、工具检查暴露为 Tauri 命令。
//! 扫描与导出依赖 OpenCV / ONNX Runtime，待 vf-pipeline、vf-export 接入后再加入。

use std::path::Path;
use std::process::Command;

use serde::Serialize;
use vf_media::{sampled_content_hash, FfprobeBackend, MediaBackend};
use vf_project::{Project, Video};

#[derive(Serialize)]
pub struct ToolStatus {
    /// ffmpeg 版本首行；未安装时为 None。
    pub ffmpeg: Option<String>,
    pub ffprobe: Option<String>,
}

fn version_line(bin: &str) -> Option<String> {
    let out = Command::new(bin).arg("-version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()?.lines().next().map(str::to_string)
}

/// 检查 ffmpeg / ffprobe 是否可用（需要在 PATH 中）。
#[tauri::command]
fn tool_status() -> ToolStatus {
    ToolStatus {
        ffmpeg: version_line("ffmpeg"),
        ffprobe: version_line("ffprobe"),
    }
}

/// 探测视频并创建一个空项目，视频信息与内容哈希写入项目。
#[tauri::command]
fn new_project(video_path: String) -> Result<Project, String> {
    let path = Path::new(&video_path);
    let info = FfprobeBackend::default().probe(path).map_err(|e| e.to_string())?;
    let content_hash = sampled_content_hash(path).map_err(|e| e.to_string())?;
    let mut project = Project::new();
    project.video = Some(Video {
        path: video_path,
        content_hash,
        duration: info.duration,
        fps: info.fps,
        width: info.width,
        height: info.height,
    });
    Ok(project)
}

#[tauri::command]
fn load_project(path: String) -> Result<Project, String> {
    Project::load(Path::new(&path)).map_err(|e| e.to_string())
}

/// 校验后原子保存（先写临时文件再替换）。
#[tauri::command]
fn save_project(path: String, project: Project) -> Result<(), String> {
    project.save(Path::new(&path)).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![tool_status, new_project, load_project, save_project])
        .run(tauri::generate_context!())
        .expect("VeilFrame 启动失败");
}
