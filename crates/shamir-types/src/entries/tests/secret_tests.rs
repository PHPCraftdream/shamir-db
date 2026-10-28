use crate::secret::SecretString;

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
    let bytes = rmp_serde::to_vec_named(&s).unwrap();
    let back: SecretString = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back.reveal(), "secret123");
}

#[test]
fn rv_types_65dde5b8_secret_into_inner_returns_owned_value() {
    let secret = SecretString::new("secret123".to_owned());
    assert_eq!(secret.into_inner(), "secret123");
}

#[test]
fn from_str_and_string() {
    let from_lit: SecretString = "hello".into();
    assert_eq!(from_lit.reveal(), "hello");

    let from_string: SecretString = "world".to_owned().into();
    assert_eq!(from_string.reveal(), "world");
}
