use shamir_query_types::admin::{
    CreateFunctionFolderOp, CreateFunctionOp, DropFunctionOp, RenameFunctionFolderOp,
    RenameFunctionOp,
};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Create (or replace) a stored function. Returns a builder for source/wasm.
pub fn create_function(name: impl Into<String>) -> CreateFunction {
    CreateFunction {
        name: name.into(),
        source: None,
        wasm: None,
        replace: false,
        visibility: None,
        security: None,
        secret_grants: Vec::new(),
        net_grants: Vec::new(),
        hmac: None,
    }
}

/// Builder for [`CreateFunctionOp`].
pub struct CreateFunction {
    name: String,
    source: Option<String>,
    wasm: Option<String>,
    replace: bool,
    visibility: Option<String>,
    security: Option<String>,
    secret_grants: Vec<String>,
    net_grants: Vec<String>,
    hmac: Option<String>,
}

impl CreateFunction {
    /// Set the Rust source code to compile.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the pre-compiled WASM bytes (base64-encoded).
    pub fn wasm(mut self, wasm: impl Into<String>) -> Self {
        self.wasm = Some(wasm.into());
        self
    }

    /// Enable replace-if-exists semantics.
    pub fn replace(mut self) -> Self {
        self.replace = true;
        self
    }

    /// Set the function visibility (`"public"` or `"private"`).
    /// Absent → `Visibility::Private` (the historical default).
    pub fn visibility(mut self, visibility: impl Into<String>) -> Self {
        self.visibility = Some(visibility.into());
        self
    }

    /// Set the security mode (`"invoker"` or `"definer"`).
    /// Absent → `Security::Invoker`. Setting `"definer"` requires an `hmac` tag.
    pub fn security(mut self, security: impl Into<String>) -> Self {
        self.security = Some(security.into());
        self
    }

    /// Set the OS-seeded env-var secret grants. Non-empty requires BOTH
    /// `Manage(Root)` AND an `hmac` tag.
    pub fn secret_grants(mut self, grants: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.secret_grants = grants.into_iter().map(Into::into).collect();
        self
    }

    /// Set the egress allowlist for this function, intersected with the
    /// DB-wide `net_allowlist` (can only narrow, never exceed the DB
    /// ceiling). Absent/empty means NO egress for this function (task
    /// #609 — matches `secret_grants`'s restrictive-by-default precedent).
    pub fn net_grants(mut self, grants: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.net_grants = grants.into_iter().map(Into::into).collect();
        self
    }

    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_create_function(name, security, secret_grants)`.
    /// Required IFF `security == "definer"` or `secret_grants` is non-empty.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateFunction(CreateFunctionOp {
            create_function: self.name,
            source: self.source,
            wasm: self.wasm,
            replace: self.replace,
            visibility: self.visibility,
            security: self.security,
            secret_grants: self.secret_grants,
            net_grants: self.net_grants,
            hmac: self.hmac,
        })
    }
}

impl From<CreateFunction> for BatchOp {
    fn from(b: CreateFunction) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateFunction {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Drop a stored function by name. Returns a builder for optional flags.
pub fn drop_function(name: impl Into<String>) -> DropFunction {
    DropFunction {
        name: name.into(),
        if_exists: false,
    }
}

/// Builder for [`DropFunctionOp`].
pub struct DropFunction {
    name: String,
    if_exists: bool,
}

impl DropFunction {
    /// Enable `IF EXISTS` semantics: dropping a non-existent function is
    /// a silent no-op (`existed: false`) instead of an error.
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropFunction(DropFunctionOp {
            drop_function: self.name,
            if_exists: self.if_exists,
        })
    }
}

impl From<DropFunction> for BatchOp {
    fn from(b: DropFunction) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropFunction {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Rename a stored function.
pub fn rename_function(from: impl Into<String>, to: impl Into<String>) -> BatchOp {
    BatchOp::RenameFunction(RenameFunctionOp {
        rename_function: from.into(),
        to: to.into(),
    })
}

/// Create a function folder by path segments.
pub fn create_function_folder(segments: impl IntoIterator<Item = impl Into<String>>) -> BatchOp {
    BatchOp::CreateFunctionFolder(CreateFunctionFolderOp {
        create_function_folder: segments.into_iter().map(Into::into).collect(),
    })
}

/// Rename a function folder (and its descendant subtree) by path segments.
pub fn rename_function_folder(
    from: impl IntoIterator<Item = impl Into<String>>,
    to: impl IntoIterator<Item = impl Into<String>>,
) -> BatchOp {
    BatchOp::RenameFunctionFolder(RenameFunctionFolderOp {
        rename_function_folder: from.into_iter().map(Into::into).collect(),
        to: to.into_iter().map(Into::into).collect(),
    })
}
