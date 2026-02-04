use crate::types::common::{new_dash_map_wc, TDashMap};
use std::sync::atomic::{AtomicU64, Ordering};


#[derive(PartialEq, Eq, Hash, Debug)]
pub enum TouchInd {
    New(u64),
    Exists(u64),
}

impl TouchInd {
    pub fn val(&self) -> u64 {
        match self {
            TouchInd::New(n) => *n,
            TouchInd::Exists(n) => *n,
        }
    }

    pub fn is_new(&self) -> bool {
        match self {
            TouchInd::New(_) => true,
            TouchInd::Exists(_) => false,
        }
    }
}

/// A thread-safe, two-way map for interning strings into u64 IDs.
/// This is the core of the key interning mechanism.
#[derive(Debug)]
pub struct Interner {
    map_str: TDashMap<String, u64>,
    map_ind: TDashMap<u64, String>,
    current: AtomicU64,
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

impl Interner {
    /// Creates a new, empty Interner.
    pub fn new() -> Interner {
        Interner {
            map_str: new_dash_map_wc(64),
            map_ind: new_dash_map_wc(64),
            current: AtomicU64::new(1),
        }
    }

    /// Creates a new Interner from a pre-existing state.
    /// This is used to "hydrate" the interner from a persistent store.
    pub fn with_state(initial_data: Vec<(u64, String)>) -> Self {
        let map_str = new_dash_map_wc(initial_data.len());
        let map_ind = new_dash_map_wc(initial_data.len());
        let mut max_id = 0;

        for (id, key) in initial_data {
            if id > max_id {
                max_id = id;
            }
            map_str.insert(key.clone(), id);
            map_ind.insert(id, key);
        }

        Interner {
            map_str,
            map_ind,
            current: AtomicU64::new(max_id + 1),
        }
    }

    /// Gets the ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> TouchInd {
        let key = str.as_ref();
        if let Some(id) = self.map_str.get(key) {
            return TouchInd::Exists(*id);
        }
        let new_id = *self.map_str.entry(key.to_string()).or_insert_with(|| {
            let id = self.current.fetch_add(1, Ordering::SeqCst);
            self.map_ind.insert(id, key.to_string());
            id
        });

        TouchInd::New(new_id)
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
        let id1 = interner.touch_ind("hello").val();
        let id2 = interner.touch_ind("world").val();
        let id3 = interner.touch_ind("hello").val();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 1);
        assert_eq!(interner.get_str(1), Some("hello".to_string()));
        assert_eq!(interner.get_str(2), Some("world".to_string()));
        assert_eq!(interner.get_ind("world"), Some(2));
    }

    #[test]
    fn test_with_state_initialization() {
        let initial_data = vec![
            (10, "name".to_string()),
            (20, "age".to_string()),
            (30, "city".to_string()),
        ];
        let interner = Interner::with_state(initial_data);

        // Check that initial data is loaded correctly
        assert_eq!(interner.get_ind("name"), Some(10));
        assert_eq!(interner.get_str(20), Some("age".to_string()));
        assert_eq!(interner.get_ind("city"), Some(30));

        // Check that touching an existing key returns the correct ID
        let touch_existing = interner.touch_ind("name");
        assert_eq!(touch_existing, TouchInd::Exists(10));
        assert!(!touch_existing.is_new());

        // Check that the next ID is correctly assigned
        let next_id = interner.touch_ind("new_key");
        assert_eq!(next_id, TouchInd::New(31));
        assert!(next_id.is_new());
        assert_eq!(interner.current.load(Ordering::SeqCst), 32);

        // Check that an empty state works
        let empty_interner = Interner::with_state(vec![]);
        let first_id = empty_interner.touch_ind("first");
        assert_eq!(first_id, TouchInd::New(1));
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
        let results: Vec<Vec<TouchInd>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for i in 1..results.len() {
            assert_eq!(
                results[0].iter().map(|v| v.val()).collect::<Vec<_>>(),
                results[i].iter().map(|v| v.val()).collect::<Vec<_>>(),
            );
        }
        assert_eq!(interner.get_ind("a"), Some(1));
        assert_eq!(interner.get_ind("h"), Some(8));
        assert_eq!(interner.current.load(Ordering::SeqCst), 9);
    }

    #[test]
    fn test_concurrent_stress() {
        let interner = Arc::new(Interner::new());
        let num_threads = 50;
        let keys_per_thread = 100;
        let mut handles = vec![];

        for thread_id in 0..num_threads {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for i in 0..keys_per_thread {
                    let key = format!("thread_{}_key_{}", thread_id, i);
                    interner_clone.touch_ind(key);
                }
            }));
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all keys were interned correctly
        let final_count = interner.current.load(Ordering::SeqCst);
        assert_eq!(final_count, (num_threads * keys_per_thread) + 1);

        // Verify that keys from different threads were interned
        assert!(interner.get_ind("thread_0_key_0").is_some());
        assert!(interner.get_ind("thread_10_key_50").is_some());
        assert!(interner.get_ind("thread_25_key_75").is_some());
        assert!(interner.get_ind("thread_49_key_99").is_some());
    }

    #[test]
    fn test_concurrent_read_while_write() {
        let interner = Arc::new(Interner::new());
        let _num_threads = 20;
        let mut handles = vec![];

        // Writer threads
        for i in 0..10 {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for j in 0..50 {
                    let key = format!("write_{}_{}", i, j);
                    interner_clone.touch_ind(key);
                }
            }));
        }

        // Reader threads
        for _i in 0..10 {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    // Try to read various keys
                    let _ = interner_clone.get_ind("write_0_0");
                    let _ = interner_clone.get_str(1);
                    let _ = interner_clone.get_ind("nonexistent");
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have 500 unique keys (10 writers * 50 keys)
        assert_eq!(interner.current.load(Ordering::SeqCst), 501);
    }

    #[test]
    fn test_concurrent_same_key_determinism() {
        let interner = Arc::new(Interner::new());
        let num_threads = 100;
        let mut handles = vec![];

        // All threads touch the same keys
        for _ in 0..num_threads {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                let mut ids = vec![];
                for key in &["shared1", "shared2", "shared3", "shared1", "shared2"] {
                    ids.push(interner_clone.touch_ind(key).val());
                }
                ids
            }));
        }

        let results: Vec<Vec<u64>> = handles.into_iter()
            .map(|h| h.join().unwrap())
            .collect();

        // All threads should get the same IDs for the same keys
        let expected = vec![1, 2, 3, 1, 2];
        for result in results {
            assert_eq!(result, expected);
        }

        // Verify final state
        assert_eq!(interner.current.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn test_concurrent_reverse_lookup() {
        let interner = Arc::new(Interner::new());
        let num_threads = 20;
        let mut handles = vec![];

        // Populate first
        for i in 0..100 {
            let key = format!("key_{}", i);
            interner.touch_ind(key);
        }

        // Concurrent reverse lookups
        for _i in 0..num_threads {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for j in 0..100 {
                    let id = (j % 100) + 1;
                    let key = interner_clone.get_str(id);
                    assert!(key.is_some());
                    assert_eq!(key, Some(format!("key_{}", j % 100)));
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_concurrent_touch_and_get() {
        let interner = Arc::new(Interner::new());
        let num_threads = 30;
        let mut handles = vec![];

        for i in 0..num_threads {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for j in 0..50 {
                    let key = format!("key_{}_{}", i, j);

                    // Touch the key
                    let touch_result = interner_clone.touch_ind(&key);

                    // Immediately verify with get_ind
                    let get_result = interner_clone.get_ind(&key);

                    assert_eq!(Some(touch_result.val()), get_result);

                    // Also verify reverse lookup
                    let reverse = interner_clone.get_str(touch_result.val());
                    assert_eq!(reverse, Some(key));
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Total: 30 threads * 50 keys = 1500
        assert_eq!(interner.current.load(Ordering::SeqCst), 1501);
    }

    #[test]
    fn test_edge_cases_empty_and_unicode() {
        let interner = Interner::new();

        // Empty string
        let id1 = interner.touch_ind("").val();
        assert_eq!(id1, 1);
        assert_eq!(interner.get_ind(""), Some(1));
        assert_eq!(interner.get_str(1), Some("".to_string()));

        // Unicode strings
        let unicode_keys = vec![
            "привет",
            "🚀🎉🔥",
            "مرحبا",
            "مرحبا2",  // Changed from duplicate to unique key
            "😀😃😄😁",
        ];

        for key in &unicode_keys {
            interner.touch_ind(key);
        }

        // Verify unicode keys work
        assert_eq!(interner.get_ind("привет"), Some(2));
        assert_eq!(interner.get_ind("🚀🎉🔥"), Some(3));
        assert_eq!(interner.get_ind("مرحبا"), Some(4));
        assert_eq!(interner.get_str(5), Some("مرحبا2".to_string()));
        assert_eq!(interner.get_ind("😀😃😄😁"), Some(6));
    }

    #[test]
    fn test_edge_cases_very_long_keys() {
        let interner = Interner::new();

        // Very long key (10KB)
        let long_key = "a".repeat(10_000);
        let id = interner.touch_ind(&long_key).val();
        assert_eq!(id, 1);
        assert_eq!(interner.get_ind(&long_key), Some(1));
        assert_eq!(interner.get_str(1), Some(long_key));
    }

    #[test]
    fn test_concurrent_with_state() {
        let initial_data: Vec<(u64, String)> = (0..100)
            .map(|i| (i, format!("initial_{}", i)))
            .collect();

        let interner = Arc::new(Interner::with_state(initial_data));
        let num_threads = 20;
        let mut handles = vec![];

        for i in 0..num_threads {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for j in 0..50 {
                    let key = format!("thread_{}_{}", i, j);
                    interner_clone.touch_ind(key);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Initial 100 + 20*50 new = 1100
        assert_eq!(interner.current.load(Ordering::SeqCst), 1100);

        // Verify initial data still accessible
        assert_eq!(interner.get_ind("initial_0"), Some(0));
        assert_eq!(interner.get_ind("initial_99"), Some(99));
        assert_eq!(interner.get_str(0), Some("initial_0".to_string()));
    }
}
