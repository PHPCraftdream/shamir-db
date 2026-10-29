//! E2e integration tests.

#[path = "transport/access_tree_e2e.rs"]
mod access_tree_e2e;
#[path = "../common/mod.rs"]
mod common;
#[path = "../connection/duplex_e2e.rs"]
mod duplex_e2e;
#[path = "../connection/duplex_loop.rs"]
mod duplex_loop;
#[path = "../connection/framer_split.rs"]
mod framer_split;
#[path = "core/interactive_tx_e2e.rs"]
mod interactive_tx_e2e;
#[path = "transport/ipc_e2e.rs"]
mod ipc_e2e;
#[path = "transport/ipc_resume_e2e.rs"]
mod ipc_resume_e2e;
#[path = "../connection/max_connections.rs"]
mod max_connections;
#[path = "core/mvp_e2e.rs"]
mod mvp_e2e;
#[path = "core/mvp_ws_e2e.rs"]
mod mvp_ws_e2e;
#[path = "core/persistence_e2e.rs"]
mod persistence_e2e;
#[path = "transport/plain_tcp_e2e.rs"]
mod plain_tcp_e2e;
#[path = "core/quickstart_e2e.rs"]
mod quickstart_e2e;
#[path = "../connection/resume_fast_path.rs"]
mod resume_fast_path;
#[path = "transport/rv_connect_0a42f42c_capped_login_reject_e2e.rs"]
mod rv_connect_0a42f42c_capped_login_reject_e2e;
#[path = "transport/rv_transport_tcp_1257db79_tls_proxy_e2e.rs"]
mod rv_transport_tcp_1257db79_tls_proxy_e2e;
#[path = "core/shutdown_release_e2e.rs"]
mod shutdown_release_e2e;
#[path = "../connection/slow_loris.rs"]
mod slow_loris;
#[path = "../connection/slow_loris_pre_tls.rs"]
mod slow_loris_pre_tls;
#[path = "transport/subscriptions_e2e/mod.rs"]
mod subscriptions_e2e;
#[path = "core/version_cas_e2e.rs"]
mod version_cas_e2e;
