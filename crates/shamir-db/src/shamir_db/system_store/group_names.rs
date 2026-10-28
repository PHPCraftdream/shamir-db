use shamir_collections::TFxMap;

pub(super) struct GroupNames {
    pub(super) by_name: TFxMap<String, u64>,
    pub(super) by_id: TFxMap<u64, String>,
}
