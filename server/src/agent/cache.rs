//! Short-TTL in-memory LRU for idempotent tool results.
//!
//! Sized by entry count; eviction is LRU. Each entry carries an insertion
//! timestamp so `get()` enforces a TTL without a sweeper task. Keys are
//! `(tool_name, args_hash)` where the hash is a stable digest of the
//! canonical-form JSON args; the `ToolCtx::meter` wrapper computes it.
//!
//! Threading: a single `Mutex<LruCache>`. Contention is low — even in
//! parallel tool dispatch, lock-hold time is a hash-map probe, and bytes_out
//! is the only slot in the entry that's non-trivial to clone.

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lru::LruCache;
use serde_json::Value;

const DEFAULT_CAPACITY: usize = 512;
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct Entry {
    value: Value,
    inserted_at: Instant,
}

pub struct ToolCache {
    inner: Mutex<LruCache<String, Entry>>,
}

impl ToolCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LruCache::new(NonZeroUsize::new(DEFAULT_CAPACITY).unwrap())),
        }
    }

    pub fn key(tool: &str, args_hash: u64) -> String {
        format!("{tool}:{args_hash:x}")
    }

    pub fn get(&self, key: &str, ttl: Duration) -> Option<Value> {
        let mut guard = self.inner.lock().ok()?;
        let entry = guard.get(key)?;
        if entry.inserted_at.elapsed() > ttl {
            guard.pop(key);
            return None;
        }
        Some(entry.value.clone())
    }

    pub fn put(&self, key: String, value: Value) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.put(key, Entry { value, inserted_at: Instant::now() });
        }
    }
}

impl Default for ToolCache {
    fn default() -> Self { Self::new() }
}
