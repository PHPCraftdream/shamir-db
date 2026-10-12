//! Wire round-trips for the typed `changes_since` response (VIS-4c field
//! on top of the T4 payload). Encoded side uses serde mirror structs (the
//! real shape the server emits) and `rmp_serde` named msgpack — the same
//! codec as the wire. No `serde_json`.

use serde::Serialize;

use super::super::{
    ChangeOpKind, ChangesSinceEvent, ChangesSinceRecordChange, ChangesSinceResponse,
};
use num_bigint::BigInt;
use shamir_types::types::value::QueryValue;

const BIG: u64 = 9_007_199_254_740_993; // 2^53 + 1 — first unsafe-integer u64

fn sample_event(commit_version: u64) -> ChangesSinceEvent {
    ChangesSinceEvent {
        repo: "main".into(),
        commit_version,
        tx_id: 7,
        actor: shamir_types::access::Actor::User(42),
        timestamp_ns: BIG,
        changes: vec![ChangesSinceRecordChange {
            table: "items".into(),
            key: vec![0u8, 1, 2, 3],
            op: ChangeOpKind::Put,
            value: Some(vec![9u8; 4]),
        }],
    }
}

/// Server-side payload shape from BEFORE VIS-4c: no `visible_through`
/// field at all. Proves `#[serde(default)]` keeps old payloads decoding.
#[derive(Serialize)]
struct PreVis4cMirror {
    changes_since: u64,
    events: Vec<ChangesSinceEvent>,
    gap_at: Option<u64>,
}

fn decode(bytes: &[u8]) -> ChangesSinceResponse {
    rmp_serde::from_slice(bytes).expect("named msgpack decode")
}

#[test]
fn visible_through_round_trips_and_defaults_when_absent() {
    let fresh = ChangesSinceResponse {
        changes_since: 41,
        events: vec![sample_event(BIG)],
        gap_at: None,
        visible_through: Some(40),
    };
    let bytes = rmp_serde::to_vec_named(&fresh).unwrap();
    assert_eq!(decode(&bytes), fresh);

    let old = PreVis4cMirror {
        changes_since: 41,
        events: vec![sample_event(BIG)],
        gap_at: Some(38),
    };
    let bytes = rmp_serde::to_vec_named(&old).unwrap();
    let decoded = decode(&bytes);
    assert_eq!(decoded.visible_through, None);
    assert_eq!(decoded.gap_at, Some(38));
    assert_eq!(decoded.changes_since, 41);
    assert_eq!(decoded.events, fresh.events);
}

#[test]
fn u64_positions_survive_native_msgpack_exactly() {
    let response = ChangesSinceResponse {
        changes_since: BIG,
        events: vec![sample_event(u64::MAX)],
        gap_at: Some(u64::MAX),
        visible_through: Some(BIG),
    };
    let bytes = rmp_serde::to_vec_named(&response).unwrap();
    assert_eq!(decode(&bytes), response);
}

#[test]
fn record_change_key_and_value_stay_bin_on_the_wire() {
    // INDEPENDENT oracle: build the `QueryValue` map exactly as the
    // server's `to_qv` pipeline produces it from a `ChangelogEvent`
    // (key/value as `QueryValue::Bin`, `op` as the unit-variant string),
    // serialize the QueryValue (the bytes the client record carries), and
    // decode into the DTO. This fails if the mirror ever switched
    // key/value to integer arrays.
    let mut actor = shamir_types::types::common::TMap::default();
    actor.insert("User".to_string(), QueryValue::Int(42));
    let mut change = shamir_types::types::common::TMap::default();
    change.insert("table".to_string(), QueryValue::Str("items".into()));
    change.insert("key".to_string(), QueryValue::Bin(vec![0u8, 1, 2, 3]));
    change.insert("op".to_string(), QueryValue::Str("Put".into()));
    change.insert("value".to_string(), QueryValue::Bin(vec![9u8, 9, 9, 9]));
    let mut event_map = shamir_types::types::common::TMap::default();
    event_map.insert("repo".to_string(), QueryValue::Str("main".into()));
    event_map.insert("commit_version".to_string(), QueryValue::Int(1));
    event_map.insert("tx_id".to_string(), QueryValue::Int(7));
    event_map.insert("actor".to_string(), QueryValue::Map(actor));
    event_map.insert("timestamp_ns".to_string(), QueryValue::Int(BIG as i64));
    event_map.insert(
        "changes".to_string(),
        QueryValue::List(vec![QueryValue::Map(change)]),
    );
    let bytes = rmp_serde::to_vec_named(&QueryValue::Map(event_map)).unwrap();
    let decoded: ChangesSinceEvent = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, sample_event(1));
}

#[test]
fn dto_serialization_matches_the_server_adapters_exactly() {
    // OUTPUT oracle, mirroring the server's own adapters byte for byte:
    // - `serde_bytes_compat` calls serialize_bytes => key is msgpack bin
    //   (bin8 0xc4, length, payload).
    // - `serde_opt_bytes_compat` calls serialize_some(v.as_ref()); rmp-serde
    //   serializes Some transparently (NO option marker) and the &[u8] goes
    //   through collect_seq with BytesMode::Normal => a FIXARRAY of the four
    //   u8s. The DTO must behave IDENTICALLY (its adapter is the same
    //   helper), and the same bytes must round-trip back into the DTO.
    let event = sample_event(1);
    let bytes = rmp_serde::to_vec_named(&event).unwrap();
    let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    // key = [0,1,2,3]: bin8 marker, length 4, then the bytes.
    assert!(
        contains(&[0xc4, 0x04, 0x00, 0x01, 0x02, 0x03]),
        "key must serialize as msgpack bin8, bytes: {bytes:02x?}"
    );
    // value = Some([9,9,9,9]): fixarray 0x94 + four posfixints — the exact
    // shape the server's serde_opt_bytes_compat produces (an array, NOT
    // bin; Some is transparent under rmp-serde).
    assert!(
        contains(&[0x94, 0x09, 0x09, 0x09, 0x09]),
        "value must serialize exactly like the server adapter (fixarray), bytes: {bytes:02x?}"
    );
    // The same bytes must round-trip into the DTO.
    let decoded: ChangesSinceEvent = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, event);
}

#[test]
fn from_query_value_accepts_decimal_string_positions_above_i64_max() {
    // QueryValue promotes integers above i64::MAX to `Big`, which
    // serializes as a decimal string. `from_query_value` must parse that
    // form back into exact u64s.
    let high = 9_223_372_036_854_775_809u64; // i64::MAX + 2
    let mut payload = shamir_types::types::common::TMap::default();
    payload.insert(
        "changes_since".to_string(),
        QueryValue::Big(BigInt::from(high)),
    );
    payload.insert("events".to_string(), QueryValue::List(Vec::new()));
    payload.insert("gap_at".to_string(), QueryValue::Null);
    payload.insert(
        "visible_through".to_string(),
        QueryValue::Big(BigInt::from(high)),
    );
    let decoded = ChangesSinceResponse::from_query_value(&QueryValue::Map(payload)).unwrap();
    assert_eq!(decoded.changes_since, high);
    assert_eq!(decoded.visible_through, Some(high));
    assert_eq!(decoded.gap_at, None);
}

#[test]
fn from_query_value_decodes_the_server_payload() {
    // The server builds the payload via msgpack(QueryValue) round-trip;
    // mirror that pipeline here: typed -> QueryValue -> from_query_value.
    let response = ChangesSinceResponse {
        changes_since: 41,
        events: vec![sample_event(BIG)],
        gap_at: None,
        visible_through: Some(40),
    };
    let bytes = rmp_serde::to_vec_named(&response).unwrap();
    let payload: shamir_types::types::value::QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(
        ChangesSinceResponse::from_query_value(&payload).unwrap(),
        response
    );
}
