use super::active_path::ActiveCallPath;
use super::binding::{RegistryBinding, RegistryEntry};
use super::identity::CallIdentity;
use crate::builtin::Argon2idFunction;
use crate::runtime::context::{FnBatch, FnCtx};
use crate::runtime::contract::ShamirFunction;
use crate::runtime::error::{FnResult, FunctionError};
use crate::runtime::params::Params;
use shamir_collections::THasher;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;
#[cfg(test)]
use std::sync::OnceLock;

/// Test-only hook run between rename's install and retire steps.
#[cfg(test)]
type InterleaveHook = Box<dyn Fn(&FunctionRegistry) + Send + Sync>;

/// Maps an invocation name to a compiled [`ShamirFunction`].
pub struct FunctionRegistry {
    functions: scc::HashMap<String, RegistryEntry, THasher>,
    catalogue_bound: bool,
    #[cfg(test)]
    pub(crate) interleave: OnceLock<InterleaveHook>,
}

impl FunctionRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            functions: scc::HashMap::with_hasher(THasher::default()),
            catalogue_bound: false,
            #[cfg(test)]
            interleave: OnceLock::new(),
        }
    }

    /// A registry pre-loaded with the built-in functions.
    pub fn with_builtins() -> Self {
        let reg = Self::new();
        // Built-ins never collide on a fresh registry.
        let _ = reg.register("argon2id", Arc::new(Argon2idFunction));
        reg
    }

    /// A registry for functions governed by a database catalogue.
    pub fn with_catalogue_builtins() -> Self {
        let reg = Self {
            catalogue_bound: true,
            ..Self::new()
        };
        let _ = reg.register_database_builtin("argon2id", Arc::new(Argon2idFunction));
        reg
    }

    fn register_database_builtin(
        &self,
        name: impl Into<String>,
        f: Arc<dyn ShamirFunction>,
    ) -> FnResult<()> {
        let name = name.into();
        self.functions
            .insert_sync(
                name.clone(),
                RegistryEntry {
                    identity: CallIdentity::new(),
                    artifact: f,
                    artifact_id: None,
                    database_builtin: true,
                },
            )
            .map_err(|_| FunctionError::AlreadyExists(name))
    }

    pub(crate) fn is_catalogue_bound(&self) -> bool {
        self.catalogue_bound
    }

    /// Register `f` under `name`; errors if the name is taken.
    pub fn register(&self, name: impl Into<String>, f: Arc<dyn ShamirFunction>) -> FnResult<()> {
        self.register_with_artifact_id(name, f, None)
    }

    /// Register `f` under `name` with its catalogue artifact id; errors if the name is taken.
    pub fn register_with_artifact_id(
        &self,
        name: impl Into<String>,
        f: Arc<dyn ShamirFunction>,
        artifact_id: Option<String>,
    ) -> FnResult<()> {
        let name = name.into();
        self.functions
            .insert_sync(
                name.clone(),
                RegistryEntry {
                    identity: CallIdentity::new(),
                    artifact: f,
                    artifact_id,
                    database_builtin: false,
                },
            )
            .map_err(|_| FunctionError::AlreadyExists(name))
    }

    /// Register or overwrite (create-or-replace). New invocations pick up the
    /// new artifact; in-flight ones keep the `Arc` they captured. Raw host
    /// replacement is trusted: an occupied entry retains its call identity,
    /// catalogue revision (artifact id), and built-in provenance. A vacant
    /// entry gets a new identity and no catalogue artifact id. The F3 row/id
    /// check covers management writes, not raw trusted host replacement.
    pub fn replace(&self, name: impl Into<String>, f: Arc<dyn ShamirFunction>) {
        let name = name.into();
        match self.functions.entry_sync(name) {
            scc::hash_map::Entry::Occupied(mut occupied) => {
                occupied.get_mut().artifact = f;
            }
            scc::hash_map::Entry::Vacant(vacant) => {
                vacant.insert_entry(RegistryEntry {
                    identity: CallIdentity::new(),
                    artifact: f,
                    artifact_id: None,
                    database_builtin: false,
                });
            }
        }
    }

    /// Register or overwrite an artifact and its catalogue artifact id together.
    pub fn replace_with_artifact_id(
        &self,
        name: impl Into<String>,
        f: Arc<dyn ShamirFunction>,
        artifact_id: Option<String>,
    ) {
        let name = name.into();
        match self.functions.entry_sync(name) {
            scc::hash_map::Entry::Occupied(mut occupied) => {
                let entry = occupied.get_mut();
                entry.artifact = f;
                entry.artifact_id = artifact_id;
                entry.database_builtin = false;
            }
            scc::hash_map::Entry::Vacant(vacant) => {
                vacant.insert_entry(RegistryEntry {
                    identity: CallIdentity::new(),
                    artifact: f,
                    artifact_id,
                    database_builtin: false,
                });
            }
        }
    }

    /// Look up a function by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ShamirFunction>> {
        self.capture(name)
            .map(|binding| Arc::clone(binding.artifact()))
    }

    /// This capture is the entry identity and artifact pair for authorization and invocation.
    pub fn capture(&self, name: &str) -> Option<RegistryBinding> {
        self.functions
            .read_sync(name, |_, entry| RegistryBinding(entry.clone()))
    }

    /// Whether a function is registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_sync(name)
    }

    /// Drop a function. Returns `true` if it existed.
    pub fn remove(&self, name: &str) -> bool {
        self.functions.remove_sync(name).is_some()
    }

    /// Rename `from` → `to` (pure re-key, no recompile). Errors if `from` is
    /// missing or `to` is already taken.
    ///
    /// The target is installed before retiring the source, so concurrent
    /// readers never see an absence caused by the rename itself.
    pub fn rename(&self, from: &str, to: &str) -> FnResult<()> {
        let binding = self
            .capture(from)
            .ok_or_else(|| FunctionError::NotFound(from.to_string()))?;
        self.functions
            .insert_sync(to.to_string(), binding.0.clone())
            .map_err(|_| FunctionError::AlreadyExists(to.to_string()))?;

        #[cfg(test)]
        if let Some(interleave) = self.interleave.get() {
            interleave(self);
        }
        if self
            .functions
            .remove_if_sync(from, |candidate| candidate.same_binding(&binding.0))
            .is_some()
        {
            return Ok(());
        }

        self.functions
            .remove_if_sync(to, |candidate| candidate.same_binding(&binding.0));
        Err(FunctionError::NotFound(from.to_string()))
    }

    /// Snapshot of registered names.
    #[allow(clippy::disallowed_methods)] // O(N) ack: Vec-capacity sizing at snapshot, off hot path
    pub fn list(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.functions.len());
        self.functions.iter_sync(|k, _| {
            out.push(k.clone());
            true
        });
        out
    }

    /// Number of registered functions.
    #[allow(clippy::disallowed_methods)] // O(N) ack: cardinality accessor, off hot path
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Look up `name` and invoke it.
    pub async fn invoke(
        &self,
        name: &str,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> FnResult<QueryValue> {
        self.validate_context_registry(ctx)?;
        if self.catalogue_bound {
            if ctx.stored_call_resolver().is_none() {
                return Err(FunctionError::Compute(
                    "catalogue-bound invocation requires a stored-call resolver".into(),
                ));
            }
            if ctx.active_call_path().len() == 0 {
                return Err(FunctionError::Compute(
                    "catalogue-bound invocation requires an active captured root".into(),
                ));
            }
        }
        if ctx.active_call_path().len() != 0 {
            return super::stored_call::dispatch_stored_call(ctx, name, batch, params).await;
        }
        let binding = self
            .capture(name)
            .ok_or_else(|| FunctionError::NotFound(name.to_string()))?;
        self.invoke_captured(&binding, ctx, batch, params).await
    }

    fn validate_context_registry(&self, ctx: &FnCtx) -> FnResult<()> {
        if !self.catalogue_bound {
            return Ok(());
        }
        match ctx.registry() {
            Some(registry) if !std::ptr::eq(registry.as_ref(), self) => {
                Err(FunctionError::Compute(
                    "function registry does not match the invocation context".into(),
                ))
            }
            Some(_) => Ok(()),
            None => Err(FunctionError::Compute(
                "catalogue-bound invocation requires its registry in the context".into(),
            )),
        }
    }

    /// Start a captured root only from a plain context with no active call path.
    pub async fn invoke_captured(
        &self,
        binding: &RegistryBinding,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> FnResult<QueryValue> {
        self.validate_context_registry(ctx)?;
        if self.catalogue_bound && ctx.stored_call_resolver().is_none() {
            return Err(FunctionError::Compute(
                "catalogue-bound invocation requires a stored-call resolver".into(),
            ));
        }
        if ctx.active_call_path().len() != 0 {
            return Err(FunctionError::Compute(
                "cannot start a captured root with an active stored-call path".into(),
            ));
        }
        let root_context = ctx.with_call_path(ActiveCallPath::root(binding.identity().clone()));
        binding.artifact().call(&root_context, batch, params).await
    }
}

impl Default for FunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
