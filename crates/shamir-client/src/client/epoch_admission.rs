//! Version gate for ambient interner epoch work; smart pre-touch remains unconditional.

pub(crate) fn should_advertise_interner_epochs(server_version: u8, has_epochs: bool) -> bool {
    server_version >= 2 && !has_epochs
}
