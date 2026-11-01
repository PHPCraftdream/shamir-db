use shamir_types::types::record_id::RecordId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableStoreNames {
    pub data: String,
    pub info: String,
    pub history: String,
}

/// Checks the mapped UTF-8 byte lengths against fjall's 255-byte store limit.
pub(crate) fn validate_table_store_name(name: &str, id: &RecordId) -> bool {
    let names = table_store_names(name, Some(id));
    [names.data, names.info, names.history]
        .iter()
        .all(|store_name| store_name.len() <= 255)
}

pub(crate) fn table_store_names(name: &str, id: Option<&RecordId>) -> TableStoreNames {
    let suffix = id.map(|id| format!("--{id}")).unwrap_or_default();
    TableStoreNames {
        data: format!("__data__{name}{suffix}"),
        info: format!("__info__{name}{suffix}"),
        history: format!("__history__{name}{suffix}"),
    }
}
