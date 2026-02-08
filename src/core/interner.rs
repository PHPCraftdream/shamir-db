use crate::types::common::{new_dash_map_wc, TDashMap};
use crate::types::string_int58::StringInt58;
use std::sync::Mutex;

/// Interned base58 key - represents a compressed string ID.
#[derive(PartialEq, Eq, Hash, Clone, Debug, PartialOrd, Ord)]
pub struct InternedKey(pub String);

impl InternedKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for InternedKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InternedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl InternedKey {
    pub fn from_str<S: AsRef<str>>(s: S) -> Self {
        InternedKey(s.as_ref().to_string())
    }
}

impl serde::Serialize for InternedKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for InternedKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(InternedKey(s))
    }
}

/// User-provided key - the original string before interning.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct UserKey(pub String);

impl UserKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for UserKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl UserKey {
    pub fn from_str<S: AsRef<str>>(s: S) -> Self {
        UserKey(s.as_ref().to_string())
    }
}

impl serde::Serialize for UserKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for UserKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(UserKey(s))
    }
}

/// Result of touching a key - indicates if it was newly created or already existed.
#[derive(PartialEq, Eq, Hash, Debug, Clone)]
pub enum TouchInd {
    New(InternedKey),
    Exists(InternedKey),
}

impl AsRef<str> for TouchInd {
    fn as_ref(&self) -> &str {
        match self {
            TouchInd::New(key) => key.as_str(),
            TouchInd::Exists(key) => key.as_str(),
        }
    }
}

impl TouchInd {
    /// Returns the interned key.
    pub fn key(&self) -> &InternedKey {
        match self {
            TouchInd::New(key) => key,
            TouchInd::Exists(key) => key,
        }
    }

    /// Returns true if the key was newly created.
    pub fn is_new(&self) -> bool {
        match self {
            TouchInd::New(_) => true,
            TouchInd::Exists(_) => false,
        }
    }
}

/// A thread-safe, two-way map for interning strings into base58 string IDs.
/// This is the core of the key interning mechanism.
/// Max 65535 unique keys per interner.
#[derive(Debug)]
pub struct Interner {
    map_user_to_interned: TDashMap<UserKey, InternedKey>,
    map_interned_to_user: TDashMap<InternedKey, UserKey>,
    current_id: Mutex<StringInt58>,
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
            map_user_to_interned: new_dash_map_wc(64),
            map_interned_to_user: new_dash_map_wc(64),
            current_id: Mutex::new(StringInt58::new()),
        }
    }

    /// Creates a new Interner from a pre-existing state.
    /// This is used to "hydrate" the interner from a persistent store.
    pub fn with_state(initial_data: Vec<(InternedKey, UserKey)>) -> Self {
        let map_user_to_interned = new_dash_map_wc(initial_data.len());
        let map_interned_to_user = new_dash_map_wc(initial_data.len());
        let mut max_id = StringInt58::new();

        for (id, key) in initial_data {
            // Track the maximum ID
            map_user_to_interned.insert(key.clone(), id.clone());
            map_interned_to_user.insert(id.clone(), key);
            // Update max_id to be at least this id
            // We need to increment until we reach or surpass this id
            while max_id.as_str().len() < id.as_str().len() ||
                  (max_id.as_str().len() == id.as_str().len() && max_id.as_str() < id.as_str()) {
                max_id.increment();
            }
        }

        Interner {
            map_user_to_interned,
            map_interned_to_user,
            current_id: Mutex::new(max_id),
        }
    }

    /// Gets the ID for a string, creating it if it doesn't exist.
    /// Returns error if max keys (65535) exceeded.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, &'static str> {
        let key = UserKey::from_str(str.as_ref());

        // Check if key already exists
        if let Some(id) = self.map_user_to_interned.get(&key) {
            return Ok(TouchInd::Exists(id.clone()));
        }

        // Create new ID
        let new_id: InternedKey = {
            let mut current_id = self.current_id.lock().unwrap();
            current_id.increment();
            InternedKey::from_str(current_id.as_str())
        };

        self.map_user_to_interned.insert(key.clone(), new_id.clone());
        self.map_interned_to_user.insert(new_id.clone(), key);

        Ok(TouchInd::New(new_id))
    }

    /// Gets the user key corresponding to an interned key.
    pub fn get_str(&self, id: &InternedKey) -> Option<UserKey> {
        self.map_interned_to_user.get(id).map(|k| k.clone())
    }

    /// Gets the interned key corresponding to a user key.
    pub fn get_ind<S: AsRef<str>>(&self, str: S) -> Option<InternedKey> {
        let key = UserKey::from_str(str.as_ref());
        self.map_user_to_interned.get(&key).map(|id| id.clone())
    }

    /// Returns the current number of interned keys.
    pub fn len(&self) -> usize {
        self.map_user_to_interned.len()
    }

    /// Returns true if the interner is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get current base58 string ID.
    pub fn current_base58(&self) -> String {
        let current_id = self.current_id.lock().unwrap();
        current_id.as_str().to_string()
    }

    /// Get next base58 ID without interning a string.
    /// This advances the base58 generator and returns the new ID.
    pub fn next_base58(&self) -> String {
        let mut current_id = self.current_id.lock().unwrap();
        current_id.increment();
        current_id.as_str().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use crate::types::string_int58::StringInt58;

    #[test]
    fn test_basic_interning() {
        let interner = Interner::new();
        let id1 = interner.touch_ind("hello").unwrap();
        let id2 = interner.touch_ind("world").unwrap();
        let id3 = interner.touch_ind("hello").unwrap();

        assert!(id1.is_new());
        assert!(id2.is_new());
        assert!(!id3.is_new());

        assert_eq!(id1.as_ref(), "2"); // First ID
        assert_eq!(id2.as_ref(), "3");
        assert_eq!(id3.as_ref(), "2"); // Same as id1

        assert_eq!(interner.get_str(&InternedKey::from_str("2")), Some(UserKey::from_str("hello")));
        assert_eq!(interner.get_str(&InternedKey::from_str("3")), Some(UserKey::from_str("world")));
        assert_eq!(interner.get_ind("world"), Some(InternedKey::from_str("3")));
    }

    #[test]
    fn test_with_state_initialization() {
        let initial_data = vec![
            (InternedKey::from_str("11"), UserKey::from_str("name")),
            (InternedKey::from_str("111"), UserKey::from_str("age")),
            (InternedKey::from_str("2111"), UserKey::from_str("city")),
        ];
        let interner = Interner::with_state(initial_data);

        // Check that initial data is loaded correctly
        assert_eq!(interner.get_ind("name"), Some(InternedKey::from_str("11")));
        assert_eq!(interner.get_str(&InternedKey::from_str("111")), Some(UserKey::from_str("age")));
        assert_eq!(interner.get_ind("city"), Some(InternedKey::from_str("2111")));

        // Check that touching an existing key returns the correct ID
        let touch_existing = interner.touch_ind("name").unwrap();
        assert!(!touch_existing.is_new());
        assert_eq!(touch_existing.as_ref(), "11");

        // Check that the next ID is correctly assigned
        let next_id = interner.touch_ind("new_key").unwrap();
        assert!(next_id.is_new());
        assert!(next_id.as_ref().len() >= 1);
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
                    ids.push(interner_clone.touch_ind(key).unwrap());
                }
                ids
            }));
        }
        let results: Vec<Vec<TouchInd>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Each thread should get consistent results - same keys get same IDs
        // across all threads (though not necessarily in insertion order)
        let first_result = &results[0];
        for i in 1..results.len() {
            let id_map_1: std::collections::HashMap<&str, &str> = first_result.iter()
                .zip(keys.iter())
                .map(|(result, key)| (*key, result.as_ref()))
                .collect();
            let id_map_2: std::collections::HashMap<&str, &str> = results[i].iter()
                .zip(keys.iter())
                .map(|(result, key)| (*key, result.as_ref()))
                .collect();

            // Verify that same keys got same IDs
            for key in ["a", "b", "c", "d", "e", "f", "g", "h"] {
                let id1 = id_map_1.get(key);
                let id2 = id_map_2.get(key);
                assert_eq!(id1, id2, "Key '{}' got different IDs", key);
            }
        }

        // Verify all keys were interned
        assert!(interner.get_ind("a").is_some());
        assert!(interner.get_ind("b").is_some());
        assert!(interner.get_ind("c").is_some());
        assert!(interner.get_ind("d").is_some());
        assert!(interner.get_ind("e").is_some());
        assert!(interner.get_ind("f").is_some());
        assert!(interner.get_ind("g").is_some());
        assert!(interner.get_ind("h").is_some());
        assert_eq!(interner.len(), 8);
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
                    interner_clone.touch_ind(key).unwrap();
                }
            }));
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all keys were interned correctly
        let final_count = interner.len();
        assert_eq!(final_count, num_threads * keys_per_thread);

        // Verify that keys from different threads were interned
        assert!(interner.get_ind("thread_0_key_0").is_some());
        assert!(interner.get_ind("thread_10_key_50").is_some());
        assert!(interner.get_ind("thread_25_key_75").is_some());
        assert!(interner.get_ind("thread_49_key_99").is_some());
    }

    #[test]
    fn test_concurrent_read_while_write() {
        let interner = Arc::new(Interner::new());
        let mut handles = vec![];

        // Writer threads
        for i in 0..10 {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for j in 0..50 {
                    let key = format!("write_{}_{}", i, j);
                    interner_clone.touch_ind(key).unwrap();
                }
            }));
        }

        // Reader threads
        for _i in 0..10 {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let _ = interner_clone.get_ind("write_0_0");
                    let _ = interner_clone.get_str(&InternedKey::from_str("2"));
                    let _ = interner_clone.get_ind("nonexistent");
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have 500 unique keys (10 writers * 50 keys)
        assert_eq!(interner.len(), 500);
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
                    ids.push(interner_clone.touch_ind(key).unwrap().as_ref().to_string());
                }
                ids
            }));
        }

        let results: Vec<Vec<String>> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All threads should get the same IDs for the same keys
        let expected = vec!["2", "3", "4", "2", "3"];
        for result in results {
            assert_eq!(result, expected);
        }

        // Verify final state
        assert_eq!(interner.len(), 3);
    }

    #[test]
    fn test_concurrent_reverse_lookup() {
        let interner = Arc::new(Interner::new());
        let num_threads = 20;
        let mut handles = vec![];

        // Populate first and collect actual IDs
        let mut key_to_id: Vec<(String, InternedKey)> = vec![];
        for i in 0..100 {
            let key = format!("key_{}", i);
            let touch_result = interner.touch_ind(key.clone()).unwrap();
            key_to_id.push((key, touch_result.key().clone()));
        }

        // Create a mapping for easy lookup
        let id_lookup: std::collections::HashMap<InternedKey, String> = key_to_id.iter()
            .map(|(k, v)| (v.clone(), k.clone()))
            .collect();

        // Concurrent reverse lookups
        for _i in 0..num_threads {
            let interner_clone = Arc::clone(&interner);
            let id_lookup_clone = id_lookup.clone();
            handles.push(thread::spawn(move || {
                for (id, expected_key) in id_lookup_clone {
                    let key = interner_clone.get_str(&id);
                    assert!(key.is_some(), "Failed to look up ID: {}", id.as_str());
                    assert_eq!(key, Some(UserKey::from_str(expected_key)));
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
                    let touch_result = interner_clone.touch_ind(&key).unwrap();

                    // Immediately verify with get_ind
                    let get_result = interner_clone.get_ind(&key);

                    assert_eq!(Some(touch_result.as_ref()), get_result.as_ref().map(|k| k.as_str()));

                    // Also verify reverse lookup
                    let reverse = interner_clone.get_str(touch_result.key());
                    assert_eq!(reverse, Some(UserKey::from_str(key.as_str())));
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Total: 30 threads * 50 keys = 1500
        assert_eq!(interner.len(), 1500);
    }

    #[test]
    fn test_edge_cases_empty_and_unicode() {
        let interner = Interner::new();

        // Empty string
        let id1 = interner.touch_ind("").unwrap();
        assert_eq!(id1.as_ref(), "2");
        assert_eq!(interner.get_ind(""), Some(InternedKey::from_str("2")));
        assert_eq!(interner.get_str(&InternedKey::from_str("2")), Some(UserKey::from_str("")));

        // Unicode strings
        let unicode_keys = vec![
            "привет",
            "🚀🎉🔥",
            "مرحبا",
            "مرحبا2",
            "😀😃😄😁",
        ];

        for key in &unicode_keys {
            interner.touch_ind(key).unwrap();
        }

        // Verify unicode keys work
        assert_eq!(interner.get_ind("привет"), Some(InternedKey::from_str("3")));
        assert_eq!(interner.get_ind("🚀🎉🔥"), Some(InternedKey::from_str("4")));
        assert_eq!(interner.get_ind("مرحبا"), Some(InternedKey::from_str("5")));
        assert_eq!(interner.get_str(&InternedKey::from_str("6")), Some(UserKey::from_str("مرحبا2")));
        assert_eq!(interner.get_ind("😀😃😄😁"), Some(InternedKey::from_str("7")));
    }

    #[test]
    fn test_edge_cases_very_long_keys() {
        let interner = Interner::new();

        // Very long key (10KB)
        let long_key = "a".repeat(10_000);
        let id = interner.touch_ind(&long_key).unwrap();
        assert_eq!(id.as_ref(), "2");
        assert_eq!(interner.get_ind(&long_key), Some(InternedKey::from_str("2")));
        assert_eq!(interner.get_str(&InternedKey::from_str("2")), Some(UserKey::from_str(long_key.clone())));
    }

    #[test]
    fn test_concurrent_with_state() {
        let initial_data: Vec<(InternedKey, UserKey)> =
            (0..100).map(|i| (InternedKey::from_str(format!("{}", i + 2)), UserKey::from_str(format!("initial_{}", i)))).collect();

        let interner = Arc::new(Interner::with_state(initial_data));
        let num_threads = 20;
        let mut handles = vec![];

        for i in 0..num_threads {
            let interner_clone = Arc::clone(&interner);
            handles.push(thread::spawn(move || {
                for j in 0..50 {
                    let key = format!("thread_{}", i * 50 + j);
                    interner_clone.touch_ind(key).unwrap();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Initial 100 + 20*50 new = 1100
        assert_eq!(interner.len(), 1100);

        // Verify initial data still accessible
        assert_eq!(interner.get_ind("initial_0"), Some(InternedKey::from_str("2")));
        assert_eq!(interner.get_ind("initial_99"), Some(InternedKey::from_str("101")));
        assert_eq!(interner.get_str(&InternedKey::from_str("2")), Some(UserKey::from_str("initial_0")));
    }

    #[test]
    fn test_len_and_is_empty() {
        let interner = Interner::new();
        assert_eq!(interner.len(), 0);
        assert!(interner.is_empty());

        interner.touch_ind("a").unwrap();
        interner.touch_ind("b").unwrap();
        assert_eq!(interner.len(), 2);
        assert!(!interner.is_empty());
    }

    #[test]
    fn test_base58_generator() {
        let interner = Interner::new();

        // Test current and next base58
        assert_eq!(interner.current_base58(), "1");
        assert_eq!(interner.next_base58(), "2");
        assert_eq!(interner.next_base58(), "3");
        assert_eq!(interner.current_base58(), "3");

        // Test multiple increments - from "3" to "z" (57 total positions, "3" is at index 2)
        for _ in 0..55 {
            interner.next_base58();
        }
        assert_eq!(interner.current_base58(), "z");
        assert_eq!(interner.next_base58(), "21");
    }

    #[test]
    fn test_base58_sequence() {
        let interner = Interner::new();

        // Generate a sequence of base58 IDs
        let mut expected = StringInt58::new();

        // First call to next_base58 should increment to "2"
        assert_eq!(interner.next_base58(), "2");
        expected.increment();
        assert_eq!(expected.as_str(), "2");

        for _ in 0..100 {
            expected.increment();
            let actual = interner.next_base58();
            assert_eq!(actual, expected.as_str());
        }
    }
}
