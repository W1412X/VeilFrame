//! 视频内容哈希（ADR-0006）：文件长度 + 首、中、尾三段采样，BLAKE3。
//! 完整读取几十 GB 的视频做哈希太慢，抽样足以区分不同视频，同一文件结果稳定。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::MediaError;

/// 每个采样块的大小。
pub const SAMPLE_CHUNK: u64 = 64 * 1024 * 1024;

/// 返回形如 `blake3-sampled:<hex>` 的哈希字符串。
pub fn sampled_content_hash(path: &Path) -> Result<String, MediaError> {
    let io = |e: std::io::Error| MediaError::ProbeFailed(format!("读取 {} 失败：{e}", path.display()));
    let mut f = File::open(path).map_err(io)?;
    let len = f.metadata().map_err(io)?.len();
    let chunk = SAMPLE_CHUNK.min(len);

    let mut offsets = vec![0, len.saturating_sub(chunk) / 2, len.saturating_sub(chunk)];
    offsets.sort_unstable();
    offsets.dedup();

    let mut hasher = blake3::Hasher::new();
    hasher.update(&len.to_le_bytes());
    let mut buf = vec![0u8; chunk as usize];
    for off in offsets {
        f.seek(SeekFrom::Start(off)).map_err(io)?;
        let want = chunk.min(len - off) as usize;
        f.read_exact(&mut buf[..want]).map_err(io)?;
        hasher.update(&buf[..want]);
    }
    Ok(format!("blake3-sampled:{}", hasher.finalize().to_hex()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("vf-hash-{}-{}", std::process::id(), name));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn same_content_gives_same_hash() {
        let a = temp_file("a.bin", b"hello video");
        let b = temp_file("b.bin", b"hello video");
        assert_eq!(sampled_content_hash(&a).unwrap(), sampled_content_hash(&b).unwrap());
        assert!(sampled_content_hash(&a).unwrap().starts_with("blake3-sampled:"));
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
    }

    #[test]
    fn different_content_gives_different_hash() {
        let a = temp_file("c.bin", b"hello video");
        let b = temp_file("d.bin", b"hello videp");
        assert_ne!(sampled_content_hash(&a).unwrap(), sampled_content_hash(&b).unwrap());
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
    }

    #[test]
    fn empty_file_is_hashable() {
        let a = temp_file("e.bin", b"");
        assert!(sampled_content_hash(&a).is_ok());
        std::fs::remove_file(a).unwrap();
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(sampled_content_hash(Path::new("/definitely/not/here.mp4")).is_err());
    }
}
