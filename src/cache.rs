
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    fetched_at: u64,
    payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub payload: Value,
    pub age_secs: u64,
}

pub struct Cache {
    dir: Option<PathBuf>,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sanitize(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Cache {

    pub fn new() -> Self {
        let dir = ProjectDirs::from("", "", "coinfetch").map(|d| d.cache_dir().to_path_buf());
        Cache { dir }
    }

    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Self {
        Cache { dir: Some(dir) }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Cache { dir: None }
    }

    fn path(&self, key: &str) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{}.json", sanitize(key))))
    }

    pub fn read_stale(&self, key: &str, now: u64) -> Option<Hit> {
        let path = self.path(key)?;
        let text = fs::read_to_string(path).ok()?;
        let entry: Entry = serde_json::from_str(&text).ok()?;
        Some(Hit {
            payload: entry.payload,
            age_secs: now.saturating_sub(entry.fetched_at),
        })
    }

    pub fn read_fresh(&self, key: &str, ttl_secs: u64, now: u64) -> Option<Hit> {
        let hit = self.read_stale(key, now)?;
        (hit.age_secs < ttl_secs).then_some(hit)
    }

    pub fn write(&self, key: &str, payload: &Value, now: u64) {
        let Some(path) = self.path(key) else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let entry = Entry {
            fetched_at: now,
            payload: payload.clone(),
        };
        if let Ok(text) = serde_json::to_string(&entry) {
            let _ = fs::write(path, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "coinfetch-cache-test-{}-{}-{}",
                tag,
                std::process::id(),
                now_secs()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_fresh_entry_is_returned() {
        let dir = TempDir::new("fresh");
        let cache = Cache::at(dir.0.clone());
        cache.write("chart_bitcoin_7d", &json!({"prices": [[1, 2.0]]}), 1_000);

        let hit = cache
            .read_fresh("chart_bitcoin_7d", 60, 1_030)
            .expect("fresh hit");
        assert_eq!(hit.age_secs, 30);
        assert_eq!(hit.payload["prices"][0][1], 2.0);
    }

    #[test]
    fn an_expired_entry_misses_the_fresh_read() {
        let dir = TempDir::new("expired");
        let cache = Cache::at(dir.0.clone());
        cache.write("k", &json!({"v": 1}), 1_000);

        assert!(cache.read_fresh("k", 60, 1_061).is_none());
    }

    #[test]
    fn an_expired_entry_is_still_available_as_a_stale_fallback() {
        let dir = TempDir::new("stale");
        let cache = Cache::at(dir.0.clone());
        cache.write("k", &json!({"v": 1}), 1_000);

        let hit = cache.read_stale("k", 5_000).expect("stale hit");
        assert_eq!(hit.age_secs, 4_000);
        assert_eq!(hit.payload["v"], 1);
    }

    #[test]
    fn a_missing_entry_is_a_miss_not_an_error() {
        let dir = TempDir::new("missing");
        let cache = Cache::at(dir.0.clone());
        assert!(cache.read_stale("nope", 0).is_none());
        assert!(cache.read_fresh("nope", 60, 0).is_none());
    }

    #[test]
    fn a_disabled_cache_stores_nothing_and_never_panics() {
        let cache = Cache::disabled();
        cache.write("k", &json!({"v": 1}), 0);
        assert!(cache.read_stale("k", 0).is_none());
    }

    #[test]
    fn keys_cannot_escape_the_cache_directory() {
        let dir = TempDir::new("escape");
        let cache = Cache::at(dir.0.clone());
        cache.write("../../etc/passwd", &json!({"v": 1}), 0);

        let entries: Vec<_> = fs::read_dir(&dir.0)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, vec!["______etc_passwd.json"]);
    }
}
