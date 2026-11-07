use crate::tests::support::{admit, append_custom_section, limits_with, load, module};
use crate::AdmissionError;

const ABI: &str = "shamir.abi";
const ORIGIN: &str = "shamir.origin";

fn with_section(name: &str, payload: &[u8]) -> Vec<u8> {
    append_custom_section(&module(""), name, payload)
}

#[test]
fn abi_section_absent_defaults_to_version_one() {
    let report = admit(&module(""), limits_with(|_| {})).unwrap();
    assert_eq!(report.abi_version, 1);
    assert_eq!(report.origin_claim, None);
}

#[test]
fn abi_section_version_one_admitted() {
    let bytes = with_section(ABI, &1u32.to_le_bytes());
    let report = admit(&bytes, limits_with(|_| {})).unwrap();
    assert_eq!(report.abi_version, 1);
}

#[test]
fn abi_section_duplicate_rejected() {
    let once = with_section(ABI, &1u32.to_le_bytes());
    let twice = append_custom_section(&once, ABI, &1u32.to_le_bytes());
    let err = admit(&twice, limits_with(|_| {})).unwrap_err();
    assert!(matches!(err, AdmissionError::DuplicateAbiSection), "{err}");
}

#[test]
fn abi_section_of_three_bytes_rejected() {
    let err = admit(&with_section(ABI, &[1, 0, 0]), limits_with(|_| {})).unwrap_err();
    assert!(
        matches!(err, AdmissionError::AbiSectionLength { observed: 3 }),
        "{err}"
    );
}

#[test]
fn abi_section_of_five_bytes_rejected() {
    let err = admit(&with_section(ABI, &[1, 0, 0, 0, 0]), limits_with(|_| {})).unwrap_err();
    assert!(
        matches!(err, AdmissionError::AbiSectionLength { observed: 5 }),
        "{err}"
    );
}

#[test]
fn abi_section_version_two_rejected_naming_both_versions() {
    let bytes = with_section(ABI, &2u32.to_le_bytes());
    let err = admit(&bytes, limits_with(|_| {})).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::UnsupportedAbiVersion {
                found: 2,
                supported: 1
            }
        ),
        "{err}"
    );
    let message = err.to_string();
    assert!(message.contains("version 2"), "{message}");
    assert!(message.contains("supported: 1"), "{message}");
}

#[test]
fn origin_section_valid_reported_as_claim() {
    let bytes = with_section(ORIGIN, b"example/origin");
    let report = admit(&bytes, limits_with(|_| {})).unwrap();
    assert_eq!(report.origin_claim.as_deref(), Some("example/origin"));
}

#[test]
fn origin_section_invalid_utf8_rejected() {
    let err = admit(&with_section(ORIGIN, &[0xff, 0xfe]), limits_with(|_| {})).unwrap_err();
    assert!(matches!(err, AdmissionError::OriginNotUtf8), "{err}");
}

#[test]
fn origin_section_of_65_bytes_rejected() {
    let bytes = with_section(ORIGIN, &[b'a'; 65]);
    let err = admit(&bytes, limits_with(|_| {})).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::OriginTooLong {
                limit: 64,
                observed: 65
            }
        ),
        "{err}"
    );
}

#[test]
fn origin_section_of_64_bytes_admitted() {
    let bytes = with_section(ORIGIN, &[b'a'; 64]);
    let report = admit(&bytes, limits_with(|_| {})).unwrap();
    assert_eq!(report.origin_claim.map(|s| s.len()), Some(64));
}

#[test]
fn origin_section_duplicate_rejected() {
    let once = with_section(ORIGIN, b"a");
    let twice = append_custom_section(&once, ORIGIN, b"b");
    let err = admit(&twice, limits_with(|_| {})).unwrap_err();
    assert!(
        matches!(err, AdmissionError::DuplicateOriginSection),
        "{err}"
    );
}

#[test]
fn unrelated_custom_section_ignored() {
    let bytes = with_section("some.other", b"whatever payload");
    let report = admit(&bytes, limits_with(|_| {})).unwrap();
    assert_eq!(report.abi_version, 1);
    assert_eq!(report.origin_claim, None);
}

#[test]
fn abi_section_version_two_rejected_under_load() {
    let err = load(&with_section(ABI, &2u32.to_le_bytes())).unwrap_err();
    assert!(
        matches!(err, AdmissionError::UnsupportedAbiVersion { .. }),
        "{err}"
    );
}

#[test]
fn origin_section_reported_under_load() {
    let report = load(&with_section(ORIGIN, b"example/origin")).unwrap();
    assert_eq!(report.origin_claim.as_deref(), Some("example/origin"));
}
