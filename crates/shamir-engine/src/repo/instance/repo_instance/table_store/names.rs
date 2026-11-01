use shamir_types::types::record_id::RecordId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableStoreNames {
    pub data: String,
    pub info: String,
    pub history: String,
}

pub(crate) fn table_store_names(name: &str, id: Option<&RecordId>) -> TableStoreNames {
    let suffix = id.map(|id| format!("--{id}")).unwrap_or_default();
    TableStoreNames {
        data: format!("__data__{name}{suffix}"),
        info: format!("__info__{name}{suffix}"),
        history: format!("__history__{name}{suffix}"),
    }
}
