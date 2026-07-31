//! F-72 (#899, P0) — persistence round-trip compat for the NEW `state:
//! IndexState` field on `IndexDefinition` (regular/unique family) and
//! `SortedIndexDefinition` (sorted family).
//!
//! bincode 1.3.3 (this workspace's pinned version) is a positional,
//! non-self-describing format: `#[serde(default)]` on a NEW trailing field
//! does NOT rescue a read of OLD on-disk bytes — see `state.rs`'s module doc
//! (proven for index2's `IndexDescriptor` by
//! `crates/shamir-index/src/tests/index_state_compat_tests.rs`). This file
//! proves the SAME property for BOTH legacy families: a definition written
//! by a build BEFORE this task (no `state` field on disk) must decode as
//! `Ready`, matching index2's legacy-lift semantics — never a decode
//! failure, never `Building`.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;

use crate::legacy::index_info::IndexInfo;
use crate::legacy::sorted_index_manager::{SortedIndexDefinition, SortedIndexManager};
use crate::state::IndexState;

// ============================================================================
// Regular-hash family — `IndexInfo` / `IndexDefinition`
// ============================================================================

/// Pre-`state` on-disk shadow shape of `IndexDefinition`, byte-faithful to
/// the struct as it existed before F-72 added the `state` field. Serialized
/// as a `BTreeMap<u64, ..>` exactly like `IndexInfo`'s own `Serialize` impl,
/// so the resulting bytes are indistinguishable from a genuine pre-F-72
/// persisted blob.
#[derive(serde::Serialize)]
struct IndexDefinitionNoStateShadow {
    name_interned: u64,
    paths: Vec<crate::legacy::index_info_item::IndexInfoItem>,
}

/// A definition written by a build BEFORE F-72 (no `state` field on disk)
/// must decode as `Ready` via `IndexInfo::decode_bytes` — not a decode
/// failure, not `Building`. Every pre-`state` persisted index was, by
/// definition, fully built (a `Building` index could not have been
/// persisted before the field existed).
#[test]
fn regular_index_pre_state_blob_decodes_as_ready() {
    use crate::legacy::index_info_item::IndexInfoItem;
    use std::collections::BTreeMap;

    let mut map: BTreeMap<u64, IndexDefinitionNoStateShadow> = BTreeMap::new();
    map.insert(
        4001,
        IndexDefinitionNoStateShadow {
            name_interned: 4001,
            paths: vec![IndexInfoItem::new(vec![10, 20])],
        },
    );
    let bytes = bincode::serialize(&map).expect("pre-state shadow must encode");

    let decoded = IndexInfo::decode_bytes(&bytes)
        .expect("a pre-state blob must decode via the legacy-shape fallback, not error");
    let def = decoded
        .get_index(4001)
        .expect("the single persisted definition must be present after decode");
    assert_eq!(
        def.state,
        IndexState::Ready,
        "F-72: a pre-state on-disk IndexDefinition must lift to Ready, not \
         Building and not fail to decode"
    );
    assert_eq!(def.name_interned, 4001);
    assert_eq!(def.paths.len(), 1);
    assert_eq!(def.paths[0].path, vec![10, 20]);
}

/// Control: a CURRENT-shape blob (explicit `Building`) must round-trip
/// exactly, proving the fallback path is never spuriously taken for
/// current-format data.
#[test]
fn regular_index_current_shape_preserves_explicit_building() {
    use crate::legacy::index_definition::IndexDefinition;
    use crate::legacy::index_info_item::IndexInfoItem;

    let mut def = IndexDefinition::new(4002, vec![IndexInfoItem::new(vec![30])]);
    def.state = IndexState::Building;
    let info = IndexInfo::from_definitions(vec![def]);

    let bytes = bincode::serialize(&info).expect("current-shape IndexInfo must encode");
    let decoded =
        IndexInfo::decode_bytes(&bytes).expect("current-shape bytes must decode via the fast path");
    let got = decoded.get_index(4002).expect("definition must be present");
    assert_eq!(
        got.state,
        IndexState::Building,
        "a new-shape Building blob must NOT fall back to the legacy path and \
         must preserve the explicit Building state"
    );
}

/// A genuinely corrupt blob (decodes as neither shape) must surface an
/// error, never silently produce an empty or lifted-to-Ready result.
#[test]
fn regular_index_corrupt_blob_errors() {
    let garbage = b"NOT_A_VALID_BINCODE_PAYLOAD_AT_ALL_______________";
    assert!(IndexInfo::decode_bytes(garbage).is_err());
}

// ============================================================================
// Sorted family — `SortedIndexManager` / `SortedIndexDefinition`
// ============================================================================

/// Pre-`state` on-disk shadow shape of `SortedIndexDefinition`: the layout as
/// it existed with `included_fields` + `ready_at_version` but BEFORE `state`
/// was added (mirrors `SortedIndexDefinitionNoState` in
/// `sorted_index_definition.rs`, re-declared here so the test hand-encodes
/// independently of the production fallback type).
#[derive(serde::Serialize)]
struct SortedIndexDefinitionNoStateShadow {
    name_interned: u64,
    field_path: Vec<u64>,
    included_fields: Vec<Vec<String>>,
    ready_at_version: u64,
}

/// A sorted definition written by a build BEFORE F-72 (no `state` field on
/// disk, but AFTER F-71's `ready_at_version`) must decode as `Ready` via
/// `SortedIndexManager::load()` (exercised through `SortedIndexManager::new`,
/// which calls `load()` internally) — not a decode failure, not `Building`.
#[tokio::test]
async fn sorted_index_pre_state_blob_decodes_as_ready() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let defs = vec![SortedIndexDefinitionNoStateShadow {
        name_interned: 5001,
        field_path: vec![40],
        included_fields: Vec::new(),
        ready_at_version: 999,
    }];
    let bytes = bincode::serialize(&defs).expect("pre-state shadow must encode");
    let sys_id = RecordId::system("sorted_indexes");
    info_store
        .set(sys_id.to_bytes().into(), bytes::Bytes::from(bytes))
        .await
        .unwrap();

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .expect("SortedIndexManager::new must decode a pre-state blob, not error");
    let def = mgr
        .find_by_name_interned(5001)
        .expect("the persisted definition must be present after load");
    assert_eq!(
        def.state,
        IndexState::Ready,
        "F-72: a pre-state on-disk SortedIndexDefinition must lift to Ready, \
         not Building and not fail to decode"
    );
    assert_eq!(
        def.ready_at_version, 999,
        "the F-71 ready_at_version field must survive the fallback decode \
         unchanged (this blob predates ONLY `state`, not `ready_at_version`)"
    );

    // The planner-facing lookup must ALSO see it (Ready, not filtered out).
    assert!(
        mgr.find_by_field_ready(&[40]).is_some(),
        "a pre-state (lifted-to-Ready) sorted definition must be visible to \
         the planner Ready-gate"
    );
}

/// Control: a CURRENT-shape blob (explicit `Building`) must round-trip
/// exactly through `SortedIndexManager::load()`.
#[tokio::test]
async fn sorted_index_current_shape_preserves_explicit_building() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let mut def = SortedIndexDefinition::new(5002, vec![50]);
    def.state = IndexState::Building;
    let bytes = bincode::serialize(&vec![def]).expect("current-shape defs must encode");
    let sys_id = RecordId::system("sorted_indexes");
    info_store
        .set(sys_id.to_bytes().into(), bytes::Bytes::from(bytes))
        .await
        .unwrap();

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .expect("current-shape bytes must decode via the fast path");
    let got = mgr
        .find_by_name_interned(5002)
        .expect("definition must be present");
    assert_eq!(
        got.state,
        IndexState::Building,
        "a new-shape Building blob must NOT fall back to a legacy path and \
         must preserve the explicit Building state"
    );
    assert!(
        mgr.find_by_field_ready(&[50]).is_none(),
        "a Building sorted definition must stay invisible to the planner \
         Ready-gate even immediately after a fresh load"
    );
}

/// A genuinely corrupt blob (decodes as neither the current shape, the
/// pre-state shape, nor the original V1 shape) must surface an error from
/// `SortedIndexManager::new`, never silently produce an empty index set.
#[tokio::test]
async fn sorted_index_corrupt_blob_errors() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let sys_id = RecordId::system("sorted_indexes");
    info_store
        .set(
            sys_id.to_bytes().into(),
            bytes::Bytes::from_static(b"NOT_A_VALID_BINCODE_PAYLOAD_AT_ALL_______________"),
        )
        .await
        .unwrap();

    let result = SortedIndexManager::new(Arc::clone(&info_store)).await;
    assert!(
        result.is_err(),
        "a corrupt sorted-index blob must surface an error, not silently \
         decode as an empty index set"
    );
}
