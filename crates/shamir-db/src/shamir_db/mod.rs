mod gateway;
pub mod system_store;
#[cfg(test)]
mod tests;

pub(crate) mod execute;
#[path = "execute/repo_admin/repo_durability/mod.rs"]
pub(crate) mod repo_durability;
#[allow(clippy::module_inception)]
pub mod shamir_db;

pub use gateway::curl_gateway;
pub use gateway::curl_gateway::CurlNetGateway;
pub use gateway::ports;
pub use gateway::ports::{
    PortError, PrincipalInfo, PrincipalResolver, RepoAdminObserver, UserAdminPort,
};
pub use shamir_db::ChangelogDeliveryGate;
pub use shamir_db::ChangelogReceiver;
pub use shamir_db::FunctionSource;
pub use shamir_db::ShamirDb;
pub use system_store::{SystemStore, SystemStoreConfig};
