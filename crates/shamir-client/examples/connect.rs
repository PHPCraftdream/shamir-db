use shamir_client::{Client, ConnectOptions};
use zeroize::Zeroizing;

async fn demo() -> Result<(), shamir_client::ClientError> {
    let client = Client::connect(ConnectOptions {
        addr: "127.0.0.1:3742".parse().unwrap(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(b"correct horse battery staple".to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await?;
    client.ping().await?;
    client.close().await;
    Ok(())
}

fn main() {
    let _ = demo;
}
