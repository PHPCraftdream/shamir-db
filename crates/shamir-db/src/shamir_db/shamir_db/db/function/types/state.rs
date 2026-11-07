/// Runtime availability of a registered function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionState {
    /// Compiled and callable.
    Loaded,
    /// Stored module failed the load policy or compilation at boot.
    Unloadable {
        /// Why the module could not be loaded.
        reason: String,
    },
    /// Stored bytes do not match the stored hash.
    Corrupt {
        /// Hash recorded in the row.
        expected: String,
        /// Hash computed from the stored bytes.
        actual: String,
    },
}

impl FunctionState {
    /// `true` only for [`FunctionState::Loaded`].
    pub fn is_available(&self) -> bool {
        matches!(self, FunctionState::Loaded)
    }

    /// Error text for an unavailable function; `None` when loaded.
    pub fn unavailable_message(&self, name: &str) -> Option<String> {
        match self {
            FunctionState::Loaded => None,
            FunctionState::Unloadable { reason } => {
                Some(format!("function '{name}' is unavailable: {reason}"))
            }
            FunctionState::Corrupt { expected, actual } => Some(format!(
                "function '{name}' is unavailable: stored module is corrupt (expected hash {expected}, actual {actual})"
            )),
        }
    }
}
