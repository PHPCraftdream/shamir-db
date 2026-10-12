use std::fs;

use crate::replication_seed::install::bounded::read_bounded;

#[test]
fn bounded_metadata_read_refuses_before_decoding() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("metadata");
    fs::write(&path, [1, 2, 3, 4]).unwrap();
    assert!(read_bounded(&path, 3)
        .unwrap_err()
        .to_string()
        .contains("byte budget"));
    assert_eq!(read_bounded(&path, 4).unwrap(), vec![1, 2, 3, 4]);
    assert!(read_bounded(&path, u64::MAX)
        .unwrap_err()
        .to_string()
        .contains("overflow"));
}
