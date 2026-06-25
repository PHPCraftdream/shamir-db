use shamir_query_types::admin::ResourceRef;

/// Reference a database by name.
pub fn database(name: impl Into<String>) -> ResourceRef {
    ResourceRef::Database {
        database: name.into(),
    }
}

/// Reference a store (repo) by `[db, store]`.
pub fn store(db: impl Into<String>, store: impl Into<String>) -> ResourceRef {
    ResourceRef::Store {
        store: [db.into(), store.into()],
    }
}

/// Reference a table by `[db, store, table]`.
pub fn table(
    db: impl Into<String>,
    store: impl Into<String>,
    table: impl Into<String>,
) -> ResourceRef {
    ResourceRef::Table {
        table: [db.into(), store.into(), table.into()],
    }
}

/// Reference a function by name.
pub fn function(name: impl Into<String>) -> ResourceRef {
    ResourceRef::Function {
        function: name.into(),
    }
}

/// Reference a function folder by its path segments.
///
/// Mirrors the TS builder's `refFunctionFolder(segments)`. The segments
/// form the folder path (e.g. `["reports", "daily"]`).
pub fn function_folder<I, S>(segments: I) -> ResourceRef
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    ResourceRef::FunctionFolder {
        function_folder: segments.into_iter().map(Into::into).collect(),
    }
}

/// Reference the function namespace singleton.
pub fn function_namespace() -> ResourceRef {
    ResourceRef::FunctionNamespace {
        function_namespace: true,
    }
}
