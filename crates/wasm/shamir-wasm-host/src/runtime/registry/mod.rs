//! Registry manifests and stored-call dispatch.

mod active_path;
mod binding;
mod function_registry;
mod identity;
mod resolver;
mod stored_call;

pub(crate) use active_path::ActiveCallPath;
pub use binding::RegistryBinding;
pub use function_registry::FunctionRegistry;
pub use resolver::{ResolvedStoredCall, StoredCallResolver};
pub(crate) use stored_call::dispatch_stored_call;
