use super::InternerKey;

/// Result of touching a key - indicates if it was newly created or already existed.
#[derive(PartialEq, Eq, Hash, Debug, Clone)]
pub enum TouchInd {
    New(InternerKey),
    Exists(InternerKey),
}

impl AsRef<[u8]> for TouchInd {
    fn as_ref(&self) -> &[u8] {
        match self {
            TouchInd::New(key) => key.as_bytes(),
            TouchInd::Exists(key) => key.as_bytes(),
        }
    }
}

impl TouchInd {
    /// Returns the interned key.
    pub fn key(&self) -> &InternerKey {
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
