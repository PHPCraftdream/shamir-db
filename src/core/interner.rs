use crate::types::common::{new_dash_map_wc, TDashMap};
use std::sync::atomic::{AtomicU64, Ordering};

/// A thread-safe, two-way map for interning strings into u64 IDs.
/// This is the core of the key interning mechanism.
#[derive(Debug)]
pub struct Interner {
    map_str: TDashMap<String, u64>,
    map_ind: TDashMap<u64, String>,
    current: AtomicU64,
}

impl Interner {
    /// Creates a new, empty NameInd.
    pub fn new() -> Interner {
        Interner {
            map_str: new_dash_map_wc(1024),
            map_ind: new_dash_map_wc(1024),
            current: AtomicU64::new(1),
        }
    }

    /// Gets the ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> u64 {
        let key = str.as_ref();
        if let Some(id) = self.map_str.get(key) {
            return *id;
        }
        let new_id = *self.map_str.entry(key.to_string()).or_insert_with(|| {
            let id = self.current.fetch_add(1, Ordering::SeqCst);
            self.map_ind.insert(id, key.to_string());
            id
        });
        new_id
    }

    /// Gets the string corresponding to an ID.
    pub fn get_str(&self, index: u64) -> Option<String> {
        self.map_ind.get(&index).map(|s| s.clone())
    }

    /// Gets the ID corresponding to a string.
    pub fn get_ind<S: AsRef<str>>(&self, str: S) -> Option<u64> {
        self.map_str.get(str.as_ref()).map(|id| *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_basic_interning() {
        let interner = Interner::new();
        let id1 = interner.touch_ind("hello");
        let id2 = interner.touch_ind("world");
        let id3 = interner.touch_ind("hello");
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 1);
        assert_eq!(interner.get_str(1), Some("hello".to_string()));
        assert_eq!(interner.get_str(2), Some("world".to_string()));
        assert_eq!(interner.get_ind("world"), Some(2));
    }

    #[test]
    fn test_concurrent_interning() {
        let interner = Arc::new(Interner::new());
        let mut handles = vec![];
        let keys = vec!["a", "b", "c", "d", "a", "e", "b", "f", "g", "h"];
        for _ in 0..10 {
            let interner_clone = Arc::clone(&interner);
            let keys_clone = keys.clone();
            handles.push(thread::spawn(move || {
                let mut ids = vec![];
                for key in keys_clone {
                    ids.push(interner_clone.touch_ind(key));
                }
                ids
            }));
        }
        let results: Vec<Vec<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for i in 1..results.len() {
            assert_eq!(results[0], results[i]);
        }
        assert_eq!(interner.get_ind("a"), Some(1));
        assert_eq!(interner.get_ind("h"), Some(8));
        assert_eq!(interner.current.load(Ordering::SeqCst), 9);
    }
}
