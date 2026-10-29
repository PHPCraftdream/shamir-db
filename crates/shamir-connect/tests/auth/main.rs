//! Single integration binary for all shamir-connect topic folders, with one module per former test target.

#[path = "../admin/integration_admin.rs"]
mod integration_admin;
#[path = "../lifecycle/integration_bootstrap.rs"]
mod integration_bootstrap;
#[path = "../lifecycle/integration_changepw.rs"]
mod integration_changepw;
mod integration_full_auth;
#[path = "../audit/integration_log_redaction.rs"]
mod integration_log_redaction;
#[path = "../resume/mod.rs"]
mod integration_resume;
#[path = "../lifecycle/integration_rotation.rs"]
mod integration_rotation;
#[path = "../session/integration_session.rs"]
mod integration_session;
#[path = "../identity/integration_username_precis.rs"]
mod integration_username_precis;
mod rv_client_f20c09bf_rotation_recovery;
#[path = "../lifecycle/rv_feature_fed6910e_client_only_surface.rs"]
mod rv_feature_fed6910e_client_only;
#[path = "rv_handshake_0fc58d93_tofu_pin_after_identity_verify.rs"]
mod rv_handshake_0fc58d93_tofu_pin;
