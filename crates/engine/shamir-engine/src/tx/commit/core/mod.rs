//! `RepoCore`: the tx-layer aggregate of a repository's shared state
//! (ENGINE-LAYERING R2c1, design §6.4); constructed by the `RepoInstance`
//! shell and handed down instead of `&RepoInstance`.

pub(crate) mod catalog_access;
pub(crate) mod durability_access;
pub(crate) mod journal_access;
pub(crate) mod repo_core;
pub(crate) mod tx_access;
pub(crate) mod tx_horizon;

pub(crate) use durability_access::timed_stage;
pub(crate) use repo_core::RepoCore;
pub(crate) use tx_horizon::TxHorizon;
