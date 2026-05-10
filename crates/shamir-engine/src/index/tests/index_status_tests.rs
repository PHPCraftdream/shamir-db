use crate::index::index_status::IndexStatus;

#[test]
fn test_index_status_from_u8() {
    assert_eq!(IndexStatus::from_u8(0), IndexStatus::Actual);
    assert_eq!(IndexStatus::from_u8(1), IndexStatus::Pending);
    assert_eq!(IndexStatus::from_u8(2), IndexStatus::Saving);
    assert_eq!(IndexStatus::from_u8(255), IndexStatus::Saving);
}

#[test]
fn test_index_status_as_u8() {
    assert_eq!(IndexStatus::Actual.as_u8(), 0);
    assert_eq!(IndexStatus::Pending.as_u8(), 1);
    assert_eq!(IndexStatus::Saving.as_u8(), 2);
}

#[test]
fn test_index_status_roundtrip() {
    for status in [
        IndexStatus::Actual,
        IndexStatus::Pending,
        IndexStatus::Saving,
    ] {
        let value = status.as_u8();
        let restored = IndexStatus::from_u8(value);
        assert_eq!(status, restored);
    }
}

#[test]
fn test_index_status_equality() {
    assert_eq!(IndexStatus::Actual, IndexStatus::Actual);
    assert_eq!(IndexStatus::Pending, IndexStatus::Pending);
    assert_eq!(IndexStatus::Saving, IndexStatus::Saving);
    assert_ne!(IndexStatus::Actual, IndexStatus::Pending);
    assert_ne!(IndexStatus::Pending, IndexStatus::Saving);
}

#[test]
fn test_index_status_clone() {
    let status = IndexStatus::Pending;
    #[allow(clippy::clone_on_copy)]
    let cloned = status.clone();
    assert_eq!(status, cloned);
}
