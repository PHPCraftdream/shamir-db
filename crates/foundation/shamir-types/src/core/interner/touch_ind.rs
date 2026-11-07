use super::InternerKey;

/// Result of touching a key - indicates if it was newly created or already existed.
#[derive(PartialEq, Eq, Hash, Debug, Clone)]
pub enum TouchInd {
    New(InternerKey),
    Exists(InternerKey),
}

impl TouchInd {
    /// Returns the interned key.
    pub fn key(&self) -> &InternerKey {
        match self {
            TouchInd::New(key) => key,
            TouchInd::Exists(key) => key,
        }
    }

    /// Consume the variant and return the owned InternerKey.
    pub fn into_key(self) -> InternerKey {
        match self {
            TouchInd::Exists(k) | TouchInd::New(k) => k,
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
