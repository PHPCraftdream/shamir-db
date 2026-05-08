//! Tests for type enums (spec §4.2 enum extension rule: unknown → fail-closed).

use crate::common::types::{BindingMode, TransportKind};

#[test]
fn transport_kind_round_trip_known_values() {
    for kind in [TransportKind::Tcp, TransportKind::WebSocket] {
        let byte = kind.as_u8();
        let parsed = TransportKind::from_u8(byte).unwrap();
        assert_eq!(parsed, kind);
    }
}

#[test]
fn transport_kind_rejects_unknown_values() {
    for byte in [0x00u8, 0x03, 0x10, 0xff] {
        assert!(
            TransportKind::from_u8(byte).is_err(),
            "byte 0x{:02x} should reject",
            byte
        );
    }
}

#[test]
fn binding_mode_round_trip_known_values() {
    for mode in [
        BindingMode::None,
        BindingMode::TlsExporter,
        BindingMode::TlsNoExport,
    ] {
        let byte = mode.as_u8();
        let parsed = BindingMode::from_u8(byte).unwrap();
        assert_eq!(parsed, mode);
    }
}

#[test]
fn binding_mode_rejects_unknown_values() {
    for byte in [0x03u8, 0x10, 0xff] {
        assert!(
            BindingMode::from_u8(byte).is_err(),
            "byte 0x{:02x} should reject",
            byte
        );
    }
}

#[test]
fn binding_mode_strength_ordering_per_spec_session_resumption_6_1() {
    // Anti-downgrade rule: higher strength is "stronger".
    assert!(BindingMode::None.strength() < BindingMode::TlsNoExport.strength());
    assert!(BindingMode::TlsNoExport.strength() < BindingMode::TlsExporter.strength());
}

#[test]
fn binding_mode_strength_concrete_values() {
    // Pinned values per spec — changing breaks downgrade math everywhere.
    assert_eq!(BindingMode::None.strength(), 0);
    assert_eq!(BindingMode::TlsNoExport.strength(), 1);
    assert_eq!(BindingMode::TlsExporter.strength(), 2);
}
