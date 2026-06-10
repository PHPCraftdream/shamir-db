use shamir_query_types::admin::{
    CreateFunctionFolderOp, CreateFunctionOp, DropFunctionOp, RenameFunctionOp,
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
    }
}

/// Builder for [`CreateFunctionOp`].
pub struct CreateFunction {
    name: String,
    source: Option<String>,
    wasm: Option<String>,
    replace: bool,
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

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateFunction(CreateFunctionOp {
            create_function: self.name,
            source: self.source,
            wasm: self.wasm,
            replace: self.replace,
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

/// Drop a stored function by name.
pub fn drop_function(name: impl Into<String>) -> BatchOp {
    BatchOp::DropFunction(DropFunctionOp {
        drop_function: name.into(),
    })
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
