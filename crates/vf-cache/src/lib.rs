//! 感知缓存（SQLite，对应 `first_design.md` 2.8）。
//!
//! 缓存键 = 视频内容哈希 + 引擎版本 + 参数指纹。键的任何一部分变化，旧结果都不再命中，
//! 从而保证结果可复现、失效正确。

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use vf_fusion::Detection;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("数据库错误：{0}")]
    Db(#[from] rusqlite::Error),
    #[error("缓存数据损坏：{0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, CacheError>;

/// 缓存命名空间：一组固定的（视频、引擎版本、参数）组合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    key: String,
}

impl Namespace {
    pub fn new(video_hash: &str, engine_version: &str, params: &str) -> Self {
        let fingerprint = blake3::hash(params.as_bytes()).to_hex();
        Self { key: format!("{video_hash}|{engine_version}|{fingerprint}") }
    }

    pub fn as_str(&self) -> &str {
        &self.key
    }
}

pub struct PerceptualCache {
    conn: Connection,
}

impl PerceptualCache {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS frame_detections (
                 ns TEXT NOT NULL,
                 frame INTEGER NOT NULL,
                 payload TEXT NOT NULL,
                 PRIMARY KEY (ns, frame)
             );
             CREATE TABLE IF NOT EXISTS scene_cuts (
                 video_hash TEXT NOT NULL,
                 t REAL NOT NULL,
                 PRIMARY KEY (video_hash, t)
             );",
        )?;
        Ok(Self { conn })
    }

    /// 写入（或覆盖）某一帧的检测结果。
    pub fn put_frame(&self, ns: &Namespace, frame: u64, dets: &[Detection]) -> Result<()> {
        let payload = serde_json::to_string(dets).map_err(|e| CacheError::Corrupt(e.to_string()))?;
        self.conn.execute(
            "INSERT OR REPLACE INTO frame_detections (ns, frame, payload) VALUES (?1, ?2, ?3)",
            params![ns.as_str(), frame as i64, payload],
        )?;
        Ok(())
    }

    /// 读取某一帧的检测结果。`None` 表示该帧尚未缓存。
    pub fn get_frame(&self, ns: &Namespace, frame: u64) -> Result<Option<Vec<Detection>>> {
        let payload: Option<String> = self
            .conn
            .query_row(
                "SELECT payload FROM frame_detections WHERE ns = ?1 AND frame = ?2",
                params![ns.as_str(), frame as i64],
                |row| row.get(0),
            )
            .optional()?;
        match payload {
            None => Ok(None),
            Some(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| CacheError::Corrupt(e.to_string())),
        }
    }

    /// 某个命名空间下已缓存的帧数。
    pub fn cached_frame_count(&self, ns: &Namespace) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM frame_detections WHERE ns = ?1",
            params![ns.as_str()],
            |row| row.get(0),
        )?;
        Ok(n as u64)
    }

    /// 写入场景切点（按视频哈希，与引擎无关）。
    pub fn put_scene_cuts(&mut self, video_hash: &str, cuts: &[f64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for t in cuts {
            tx.execute(
                "INSERT OR IGNORE INTO scene_cuts (video_hash, t) VALUES (?1, ?2)",
                params![video_hash, t],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 按时间升序返回场景切点。
    pub fn scene_cuts(&self, video_hash: &str) -> Result<Vec<f64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT t FROM scene_cuts WHERE video_hash = ?1 ORDER BY t")?;
        let rows = stmt.query_map(params![video_hash], |row| row.get::<_, f64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_project::Quad;

    fn det(conf: f64) -> Detection {
        let q: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        Detection { quad: q, conf }
    }

    #[test]
    fn frame_round_trip() {
        let cache = PerceptualCache::open_in_memory().unwrap();
        let ns = Namespace::new("h1", "sift-1", "{\"thr\":0.6}");
        cache.put_frame(&ns, 12, &[det(0.9)]).unwrap();
        assert_eq!(cache.get_frame(&ns, 12).unwrap(), Some(vec![det(0.9)]));
        assert_eq!(cache.get_frame(&ns, 13).unwrap(), None);
        assert_eq!(cache.cached_frame_count(&ns).unwrap(), 1);
    }

    #[test]
    fn overwrite_replaces_previous_result() {
        let cache = PerceptualCache::open_in_memory().unwrap();
        let ns = Namespace::new("h1", "sift-1", "p");
        cache.put_frame(&ns, 1, &[det(0.5)]).unwrap();
        cache.put_frame(&ns, 1, &[det(0.8), det(0.7)]).unwrap();
        assert_eq!(cache.get_frame(&ns, 1).unwrap().unwrap().len(), 2);
        assert_eq!(cache.cached_frame_count(&ns).unwrap(), 1);
    }

    #[test]
    fn changing_engine_version_or_params_misses() {
        let cache = PerceptualCache::open_in_memory().unwrap();
        let base = Namespace::new("h1", "sift-1", "p");
        cache.put_frame(&base, 5, &[det(0.9)]).unwrap();

        let new_engine = Namespace::new("h1", "sift-2", "p");
        let new_params = Namespace::new("h1", "sift-1", "p2");
        let new_video = Namespace::new("h2", "sift-1", "p");
        assert_eq!(cache.get_frame(&new_engine, 5).unwrap(), None);
        assert_eq!(cache.get_frame(&new_params, 5).unwrap(), None);
        assert_eq!(cache.get_frame(&new_video, 5).unwrap(), None);
    }

    #[test]
    fn scene_cuts_are_sorted_and_deduplicated() {
        let mut cache = PerceptualCache::open_in_memory().unwrap();
        cache.put_scene_cuts("h1", &[30.0, 10.0, 30.0]).unwrap();
        assert_eq!(cache.scene_cuts("h1").unwrap(), vec![10.0, 30.0]);
        assert!(cache.scene_cuts("other").unwrap().is_empty());
    }

    #[test]
    fn corrupt_payload_is_reported() {
        let cache = PerceptualCache::open_in_memory().unwrap();
        let ns = Namespace::new("h1", "e", "p");
        cache
            .conn
            .execute(
                "INSERT INTO frame_detections (ns, frame, payload) VALUES (?1, 1, 'not json')",
                params![ns.as_str()],
            )
            .unwrap();
        assert!(matches!(cache.get_frame(&ns, 1), Err(CacheError::Corrupt(_))));
    }
}
