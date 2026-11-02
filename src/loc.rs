//! Caches for where things live on the destinations: positive lookups (path →
//! destination + attrs), short negative lookups, and merged directory listings.
//! They keep hot-path lookups (every create triggers one) off the network.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::proto::{Entry, Kind};

// Files on destinations rarely change, but something outside spillway (a
// retention job) may delete them: `open` revalidates entries older than a
// couple of seconds, and the TTL bounds how long `stat` can be stale.
const POS_FILE_TTL: Duration = Duration::from_secs(30);
const POS_DIR_TTL: Duration = Duration::from_secs(5);
const NEG_TTL: Duration = Duration::from_millis(1000);
const LIST_TTL: Duration = Duration::from_secs(2);
const MAX_POS: usize = 500_000;
const MAX_NEG: usize = 200_000;
const MAX_LISTS: usize = 2_000;

#[derive(Clone, Debug)]
pub struct Loc {
    pub dest: String,
    pub entry: Entry,
}

/// Directory listing merged across destinations: name → (first dest that has it, entry).
pub type Listing = Vec<(String, Entry)>;

#[derive(Default)]
pub struct LocCache {
    pos: Mutex<HashMap<String, (Loc, Instant)>>,
    neg: Mutex<HashMap<String, Instant>>,
    lists: Mutex<HashMap<String, (Listing, Instant)>>,
}

impl LocCache {
    pub fn get(&self, rel: &str) -> Option<Loc> {
        self.get_aged(rel).map(|(l, _)| l)
    }

    /// Like `get`, plus how long ago the entry was confirmed.
    pub fn get_aged(&self, rel: &str) -> Option<(Loc, Duration)> {
        let g = self.pos.lock();
        let (loc, at) = g.get(rel)?;
        let ttl = if loc.entry.kind == Kind::Dir { POS_DIR_TTL } else { POS_FILE_TTL };
        let age = at.elapsed();
        (age < ttl).then(|| (loc.clone(), age))
    }

    pub fn put(&self, rel: &str, dest: &str, entry: Entry) {
        let mut g = self.pos.lock();
        if g.len() >= MAX_POS {
            g.clear();
        }
        g.insert(rel.to_string(), (Loc { dest: dest.to_string(), entry }, Instant::now()));
        drop(g);
        self.neg.lock().remove(rel);
    }

    pub fn is_negative(&self, rel: &str) -> bool {
        self.neg.lock().get(rel).is_some_and(|t| t.elapsed() < NEG_TTL)
    }

    pub fn put_negative(&self, rel: &str) {
        let mut g = self.neg.lock();
        if g.len() >= MAX_NEG {
            g.retain(|_, t| t.elapsed() < NEG_TTL);
            if g.len() >= MAX_NEG {
                g.clear();
            }
        }
        g.insert(rel.to_string(), Instant::now());
    }

    /// Forget everything known about `rel` (and its parent's listing).
    pub fn invalidate(&self, rel: &str) {
        self.pos.lock().remove(rel);
        self.neg.lock().remove(rel);
        let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
        self.lists.lock().remove(parent);
    }

    pub fn invalidate_dest(&self, dest: &str) {
        self.pos.lock().retain(|_, (l, _)| l.dest != dest);
        self.lists.lock().clear();
    }

    pub fn clear_negative(&self) {
        self.neg.lock().clear();
    }

    pub fn listing(&self, dir: &str) -> Option<Listing> {
        let g = self.lists.lock();
        let (l, at) = g.get(dir)?;
        (at.elapsed() < LIST_TTL).then(|| l.clone())
    }

    pub fn put_listing(&self, dir: &str, l: Listing) {
        let mut g = self.lists.lock();
        if g.len() >= MAX_LISTS {
            g.clear();
        }
        g.insert(dir.to_string(), (l, Instant::now()));
    }

    pub fn len(&self) -> (usize, usize) {
        (self.pos.lock().len(), self.neg.lock().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: Kind) -> Entry {
        Entry { name: "x".into(), kind, size: 1, mtime_ms: 0, mode: 0o644, uid: 0, gid: 0 }
    }

    #[test]
    fn positive_negative_invalidate() {
        let c = LocCache::default();
        c.put_negative("a/b");
        assert!(c.is_negative("a/b"));
        c.put("a/b", "d1", e(Kind::File));
        assert!(!c.is_negative("a/b"), "positive clears negative");
        assert_eq!(c.get("a/b").unwrap().dest, "d1");
        c.put_listing("a", vec![]);
        c.invalidate("a/b");
        assert!(c.get("a/b").is_none());
        assert!(c.listing("a").is_none());
        c.put("z", "d2", e(Kind::Dir));
        c.invalidate_dest("d2");
        assert!(c.get("z").is_none());
    }
}
