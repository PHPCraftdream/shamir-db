use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use std::convert::Infallible;
use tokio::sync::mpsc;

pub(crate) async fn response_body(
    body: Bytes,
    hold: Option<Box<dyn Send + Sync>>,
    sender: mpsc::Sender<Box<dyn Send + Sync>>,
) -> BoxBody<Bytes, Infallible> {
    if let Some(hold) = hold {
        let _ = sender.send(hold).await;
    }
    Full::new(body).boxed()
}
