//! Non-transactional write execution (`execute_*_tx`) on `TableManager`.

mod delete;
mod insert;
mod set;
mod update;

pub(crate) mod write_helpers;

#[cfg(test)]
mod tests;
