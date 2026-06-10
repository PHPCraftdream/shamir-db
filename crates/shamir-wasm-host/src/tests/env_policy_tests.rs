use crate::env_policy::glob_matches;

#[test]
fn glob_star_only() {
    assert!(glob_matches("*", "anything"));
    assert!(glob_matches("*", ""));
}

#[test]
fn glob_prefix_star() {
    assert!(glob_matches("AWS_*", "AWS_KEY"));
    assert!(glob_matches("AWS_*", "AWS_"));
    assert!(!glob_matches("AWS_*", "BWS_KEY"));
}

#[test]
fn glob_star_suffix() {
    assert!(glob_matches("*_KEY", "AWS_KEY"));
    assert!(!glob_matches("*_KEY", "AWS_VAL"));
}

#[test]
fn glob_no_star() {
    assert!(glob_matches("HOME", "HOME"));
    assert!(!glob_matches("HOME", "PATH"));
}
