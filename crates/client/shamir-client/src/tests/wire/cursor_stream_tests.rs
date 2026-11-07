//! FG-5c wire half: the client publishes the assigned cursor id BEFORE the
//! first `FetchNext` frame leaves the demux, proven against an emulated
//! server over `Client::for_test` duplex pipes.

use futures::StreamExt;
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_query_builder::query::Query;
use shamir_query_types::read::QueryResult;
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};

use crate::Client;

#[tokio::test]
async fn empty_first_page_publishes_cursor_before_fetch_completes() {
    let (client_io, mut server_io) = tokio::io::duplex(65536);
    let (reader, writer) = tokio::io::split(client_io);
    let client = Client::for_test(reader, writer, crate::client::LifecycleHooks::default());
    let mut stream = client.stream_cursor("db", Query::from("items").build(), 2);
    {
        let server = async {
            let frame = read_frame(&mut server_io, MAX_FRAME_SIZE_DEFAULT)
                .await
                .expect("create");
            let req = RequestEnvelope::from_msgpack(&frame).expect("envelope");
            let page = DbResponse::CursorPage {
                cursor_id: CursorId(19),
                page: QueryResult::records_only(vec![]),
                has_more: true,
            };
            let payload = rmp_serde::to_vec_named(&page).expect("page");
            let reply = ResponseEnvelope::ok(req.request_id, payload)
                .to_msgpack()
                .expect("response");
            write_frame(&mut server_io, &reply).await.expect("write");
            let fetch = read_frame(&mut server_io, MAX_FRAME_SIZE_DEFAULT)
                .await
                .expect("fetch");
            let envelope = RequestEnvelope::from_msgpack(&fetch).expect("fetch envelope");
            assert!(matches!(
                rmp_serde::from_slice::<DbRequest>(&envelope.req).unwrap(),
                DbRequest::FetchNext { .. }
            ));
        };
        let next = stream.next();
        tokio::pin!(next);
        tokio::select! {
            _ = server => {},
            result = &mut next => panic!("unexpected stream result: {result:?}"),
        }
    }
    assert_eq!(stream.cursor_id(), Some(CursorId(19)));
}
