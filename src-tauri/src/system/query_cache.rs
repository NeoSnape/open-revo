// OpenRevo - Generic TTL Query Cache
// Shared building blocks for cached hardware/system queries.
//
// Several background loops (telemetry heartbeat, watcher, process scanner) re-run
// expensive OS queries that only change on rare events: full display-mode enumeration
// (~22 ms on a 210-mode panel), Toolhelp32 process snapshots, and registry scans.
// Wrapping those queries in one of these caches keeps the "read" API unchanged while
// turning repeated probes into memory hits.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

/// Single-slot cache holding one value until `ttl` elapses or it is invalidated.
///
/// The lock is never held while `compute` runs: a slow probe must not block other
/// callers, and holding it across a compute that re-enters this cache would deadlock.
pub struct ExpiringCache<V> {
    ttl: Duration,
    slot: RwLock<Option<(Instant, V)>>,
}

impl<V: Clone> ExpiringCache<V> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            slot: RwLock::new(None),
        }
    }

    /// Return the cached value, or compute and store it.
    pub fn get_or_compute<F>(&self, compute: F) -> V
    where
        F: FnOnce() -> V,
    {
        if let Some((stamped, value)) = self.slot.read().as_ref() {
            if stamped.elapsed() < self.ttl {
                return value.clone();
            }
        }

        let value = compute();
        *self.slot.write() = Some((Instant::now(), value.clone()));
        value
    }

    /// Cached value if it is still fresh.
    #[allow(dead_code)]
    pub fn get(&self) -> Option<V> {
        self.slot
            .read()
            .as_ref()
            .filter(|(stamped, _)| stamped.elapsed() < self.ttl)
            .map(|(_, value)| value.clone())
    }

    /// Drop the cached value so the next call recomputes.
    pub fn invalidate(&self) {
        *self.slot.write() = None;
    }

    /// Store a known value, refreshing the timestamp (write-through after a mutation).
    #[allow(dead_code)]
    pub fn put(&self, value: V) {
        *self.slot.write() = Some((Instant::now(), value));
    }
}

/// Per-key variant of [`ExpiringCache`], for queries parameterised by device name,
/// hardware id, profile path, etc.
#[allow(dead_code)]
pub struct KeyedExpiringCache<K, V> {
    ttl: Duration,
    map: RwLock<HashMap<K, (Instant, V)>>,
}

#[allow(dead_code)]
impl<K: Eq + Hash + Clone, V: Clone> KeyedExpiringCache<K, V> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            map: RwLock::new(HashMap::new()),
        }
    }

    /// Return the cached value for `key`, or compute and store it.
    pub fn get_or_compute<F>(&self, key: K, compute: F) -> V
    where
        F: FnOnce() -> V,
    {
        if let Some((stamped, value)) = self.map.read().get(&key) {
            if stamped.elapsed() < self.ttl {
                return value.clone();
            }
        }

        let value = compute();
        self.map
            .write()
            .insert(key, (Instant::now(), value.clone()));
        value
    }

    /// Cached value for `key` if it is still fresh.
    pub fn get(&self, key: &K) -> Option<V> {
        self.map
            .read()
            .get(key)
            .filter(|(stamped, _)| stamped.elapsed() < self.ttl)
            .map(|(_, value)| value.clone())
    }

    /// Drop every entry. Use when a system-wide event invalidates all keys
    /// (display topology change, config reload and so on).
    pub fn invalidate_all(&self) {
        self.map.write().clear();
    }

    /// Drop a single entry.
    pub fn invalidate(&self, key: &K) {
        self.map.write().remove(key);
    }

    /// Store a known value, refreshing the timestamp.
    pub fn put(&self, key: K, value: V) {
        self.map.write().insert(key, (Instant::now(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn expiring_cache_hits_and_invalidates() {
        let cache = ExpiringCache::new(Duration::from_secs(60));
        let calls = AtomicUsize::new(0);
        let v1 = cache.get_or_compute(|| calls.fetch_add(1, Ordering::SeqCst) + 1);
        let v2 = cache.get_or_compute(|| calls.fetch_add(1, Ordering::SeqCst) + 1);
        assert_eq!((v1, v2), (1, 1));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        cache.invalidate();
        let v3 = cache.get_or_compute(|| calls.fetch_add(1, Ordering::SeqCst) + 1);
        assert_eq!(v3, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn keyed_expiring_cache_isolates_keys_and_clears() {
        let cache = KeyedExpiringCache::new(Duration::from_secs(60));
        assert_eq!(cache.get_or_compute("a", || 10), 10);
        assert_eq!(cache.get_or_compute("b", || 20), 20);
        assert_eq!(cache.get(&"a"), Some(10));
        cache.invalidate(&"a");
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.get(&"b"), Some(20));
        cache.invalidate_all();
        assert_eq!(cache.get(&"b"), None);
    }
}
