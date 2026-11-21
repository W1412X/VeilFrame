// 探测本机 OpenCV 主次版本，给调用方 crate 打 `opencv_ge411` cfg。
//
// 背景：OpenCV 4.11 起 `cv::cvtColor` 增加了 `AlgorithmHint` 参数，而 `opencv`
// crate 按构建机头文件生成绑定——本地/CI（4.10 / apt 4.5.4）与 Release
// （vcpkg 最新）版本不同，同一行 `cvt_color` 必须按版本分支，否则必有一边编不过。
//
// 探测顺序与 opencv crate 的实际构建方式对齐：
//   1. 显式 OPENCV_VERSION
//   2. OPENCV_INCLUDE_PATHS 指向的头文件 version.hpp（本地 WSL 走这条）
//   3. vcpkg 安装树的 opencv4.pc（Release 走这条）
//   4. pkg-config（CI apt 走这条）
// 由 crates/vf-image/build.rs 与 crates/vf-render/build.rs include 使用。

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=OPENCV_VERSION");
    println!("cargo:rerun-if-env-changed=OPENCV_INCLUDE_PATHS");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=VCPKGRS_TRIPLET");
    println!("cargo:rustc-check-cfg=cfg(opencv_ge411)");

    let (major, minor) = detect().unwrap_or_else(|| {
        panic!(
            "探测不到 OpenCV 版本：设置 OPENCV_VERSION=4.x.y，\n\
             或设置 OPENCV_INCLUDE_PATHS 指向含 opencv2/core/version.hpp 的头文件目录，\n\
             或设置 VCPKG_ROOT（可选 VCPKGRS_TRIPLET）指向 vcpkg 安装树，\n\
             或让 pkg-config 能找到 opencv4.pc"
        )
    });

    if major > 4 || (major == 4 && minor >= 11) {
        println!("cargo:rustc-cfg=opencv_ge411");
        println!("cargo:warning=OpenCV {major}.{minor} >= 4.11：cvt_color 走 AlgorithmHint 五参分支");
    }
}

fn detect() -> Option<(u32, u32)> {
    if let Ok(v) = env::var("OPENCV_VERSION") {
        if !v.trim().is_empty() {
            return Some(parse_version(&v));
        }
    }
    include_header_version()
        .or_else(vcpkg_pc_version)
        .or_else(pkg_config_version)
}

/// 本地源码安装的 OpenCV：头文件里的 CV_VERSION_MAJOR/MINOR 才是真正参与绑定的版本。
fn include_header_version() -> Option<(u32, u32)> {
    let raw = env::var_os("OPENCV_INCLUDE_PATHS")?;
    for dir in env::split_paths(&raw) {
        let Ok(text) = std::fs::read_to_string(dir.join("opencv2/core/version.hpp")) else {
            continue;
        };
        let mut major: Option<u32> = None;
        let mut minor: Option<u32> = None;
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("#define CV_VERSION_MAJOR") {
                major = rest.trim().parse().ok();
            } else if let Some(rest) = line.strip_prefix("#define CV_VERSION_MINOR") {
                minor = rest.trim().parse().ok();
            } else if let Some(rest) = line.strip_prefix("#define CV_MAJOR_VERSION") {
                if major.is_none() {
                    major = rest.trim().parse().ok();
                }
            } else if let Some(rest) = line.strip_prefix("#define CV_MINOR_VERSION") {
                if minor.is_none() {
                    minor = rest.trim().parse().ok();
                }
            }
        }
        if let (Some(ma), Some(mi)) = (major, minor) {
            return Some((ma, mi));
        }
    }
    None
}

/// vcpkg 安装树里的 `opencv4.pc`（Release 构建走这条，runner 上没有系统 OpenCV）。
fn vcpkg_pc_version() -> Option<(u32, u32)> {
    let root = PathBuf::from(env::var_os("VCPKG_ROOT")?);
    let mut dirs = Vec::new();
    if let Ok(triplet) = env::var("VCPKGRS_TRIPLET") {
        dirs.push(root.join("installed").join(triplet));
    }
    if let Ok(entries) = std::fs::read_dir(root.join("installed")) {
        for entry in entries.flatten() {
            dirs.push(entry.path());
        }
    }
    for dir in dirs {
        let Ok(text) = std::fs::read_to_string(dir.join("lib/pkgconfig/opencv4.pc")) else {
            continue;
        };
        if let Some(v) = text.lines().find_map(|l| l.strip_prefix("Version:")) {
            return Some(parse_version(v.trim()));
        }
    }
    None
}

/// apt 安装的系统 OpenCV（CI 走这条）。
fn pkg_config_version() -> Option<(u32, u32)> {
    let out = std::process::Command::new("pkg-config")
        .args(["--modversion", "opencv4"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then(|| parse_version(&v))
}

fn parse_version(v: &str) -> (u32, u32) {
    let mut nums = v.split('.').filter_map(|part| {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u32>().ok()
    });
    (nums.next().unwrap_or(0), nums.next().unwrap_or(0))
}
