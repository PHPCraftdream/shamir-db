//! Foreign-key referential-action enforcement (RESTRICT / CASCADE /
//! SET NULL) for the batch DELETE and UPDATE paths.

pub(super) mod fk_actions;
pub(super) mod fk_on_update;
pub(super) mod fk_restrict;
