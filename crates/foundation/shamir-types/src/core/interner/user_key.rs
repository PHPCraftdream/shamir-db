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

/// `Borrow<str>` lets HashMap-shape lookups (DashMap::get,
/// HashMap::get) take a plain `&str` without first allocating
/// a `UserKey` wrapper. Hot fast path on `touch_ind` /
/// `get_ind` — every cache-hit lookup used to pay one
/// `String::from(s)` to build the lookup key.
impl std::borrow::Borrow<str> for UserKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl UserKey {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str<S: AsRef<str>>(s: S) -> Self {
        UserKey(s.as_ref().to_string())
    }
}

impl std::str::FromStr for UserKey {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(UserKey(s.to_string()))
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
