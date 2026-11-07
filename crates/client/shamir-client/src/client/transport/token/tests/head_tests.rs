use super::fake_server::{FakeServer, Scripted};

const FORBIDDEN: [&str; 7] = [
    "cookie",
    "origin",
    "expect",
    "connection",
    "transfer-encoding",
    "accept-encoding",
    "user-agent",
];

#[tokio::test]
async fn post_request_head_is_exact() {
    let server = FakeServer::start(vec![Scripted::pong()]).await;
    let transport = server.transport(None, None).await;

    let reply = transport.post_request(vec![1, 2, 3]).await.unwrap();

    assert_eq!(reply.status, 200);
    let heads = server.heads();
    assert_eq!(heads.len(), 1);
    let head = &heads[0];
    assert_eq!(head.request_line, "POST /shamir/v1/request HTTP/1.1");
    assert_eq!(head.values("authorization"), vec!["Bearer shk1.test"]);
    assert_eq!(head.values("content-type"), vec!["application/msgpack"]);
    assert_eq!(head.values("accept"), vec!["application/msgpack"]);
    assert_eq!(head.values("content-length"), vec!["3"]);
    assert_eq!(
        head.values("host"),
        vec![format!("127.0.0.1:{}", server.addr.port())]
    );
    for name in FORBIDDEN {
        assert!(head.values(name).is_empty(), "unexpected header {name}");
    }
    assert_eq!(
        head.names(),
        vec![
            "accept",
            "authorization",
            "content-length",
            "content-type",
            "host",
        ]
    );
}

#[tokio::test]
async fn ping_head_is_exact() {
    let server = FakeServer::start(vec![Scripted::pong()]).await;
    let transport = server.transport(None, None).await;

    let reply = transport.get_ping().await.unwrap();

    assert_eq!(reply.status, 200);
    let heads = server.heads();
    assert_eq!(heads.len(), 1);
    let head = &heads[0];
    assert_eq!(head.request_line, "GET /shamir/v1/ping HTTP/1.1");
    assert_eq!(head.values("authorization"), vec!["Bearer shk1.test"]);
    assert_eq!(
        head.values("host"),
        vec![format!("127.0.0.1:{}", server.addr.port())]
    );
    for name in FORBIDDEN {
        assert!(head.values(name).is_empty(), "unexpected header {name}");
    }
    assert!(head.values("content-type").is_empty());
    assert!(head
        .values("content-length")
        .iter()
        .all(|value| *value == "0"));
}
