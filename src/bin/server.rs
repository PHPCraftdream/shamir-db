//! ShamirDB server binary.

use std::sync::Arc;
use shamir_db::db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::db::engine::repo::RepoConfig;
use shamir_db::db::engine::table::TableConfig;
use shamir_db::db::net::server::{self, ServerConfig};
use shamir_db::db::net::tls;
use shamir_db::db::ShamirDb;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Init ShamirDb
    let shamir = ShamirDb::init_memory().await?;
    let db = shamir.create_db("default").await;
    db.add_repo(
        RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users"))
            .add_table(TableConfig::new("orders")),
    ).await?;

    // Create test user
    let users_table = shamir.system_store().users_table().await?;
    let create_user = shamir_db::db::query::write::SetOp {
        set: shamir_db::db::query::TableRef::new("users"),
        key: serde_json::json!({"name": "admin"}),
        value: serde_json::json!({
            "name": "admin",
            "password_hash": "admin123",
            "roles": ["superadmin"]
        }),
    };
    users_table.execute_set(&create_user).await?;
    users_table.interner().persist().await?;

    // Generate TLS cert & start server
    let (acceptor, cert_pem) = tls::create_tls_config();

    // Write cert for clients
    std::fs::write("server-cert.pem", &cert_pem)?;
    log::info!("Certificate written to server-cert.pem");

    let config = ServerConfig::default();
    log::info!("Starting ShamirDB server on {}", config.addr);

    server::run_server(Arc::new(shamir), config, acceptor).await?;
    Ok(())
}
