use crate::auth::secret::SecretString;

#[test]
fn debug_redacts_value() {
    let s = SecretString::new("hunter2".to_owned());
    let dbg = format!("{:?}", s);
    assert_eq!(dbg, "SecretString(***)");
    assert!(!dbg.contains("hunter2"));
}

#[test]
fn serde_roundtrip_preserves_value() {
    let s = SecretString::new("secret123".to_owned());
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(json, "\"secret123\"");

    let back: SecretString = serde_json::from_str(&json).unwrap();
    assert_eq!(back.reveal(), "secret123");
}

#[test]
fn from_str_and_string() {
    let from_lit: SecretString = "hello".into();
    assert_eq!(from_lit.reveal(), "hello");

    let from_string: SecretString = "world".to_owned().into();
    assert_eq!(from_string.reveal(), "world");
}
