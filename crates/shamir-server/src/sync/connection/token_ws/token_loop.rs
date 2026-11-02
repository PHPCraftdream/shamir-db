//! Duplex request loop for a token-authenticated WebSocket connection.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use shamir_connect::common::envelope::RequestEnvelopeView;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::Session;
use shamir_connect::server::token::{Endpoint, Verified};
use shamir_query_types::wire::{DbRequest, DbResponse};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;

use crate::sync::connection::request_loop::WriterMsg;
use crate::sync::connection::session::writer_task::run_writer_task;
use crate::sync::connection::support::push_sink::MpscPushSink;
use crate::sync::connection::token_ws::{TokenSessionTeardown, TokenWsFrameReader, TokenWsFrameWriter};
use crate::engine::db_handler::ShamirDbHandler;
use crate::engine::subscriptions::SubscriptionRegistry;
use crate::sync::framer::{CloseIntent, FrameReader, FrameWriter};
use crate::security::perimeter::http_rest::{HandleDenial, ScopedHandler};
use crate::security::perimeter::byte_budget;
use crate::tokens::TokenService;
use shamir_transport_http::server::routes::ws::UpgradedIo;

/// Dependencies and limits for one authenticated token WebSocket.
pub(crate) struct TokenLoopContext {
    pub(crate) service: Arc<TokenService>,
    pub(crate) handler: Arc<ShamirDbHandler>,
    pub(crate) grant: Arc<Verified>,
    pub(crate) session: Arc<Session>,
    pub(crate) expires_at_ns: u64,
    pub(crate) cancel: CancellationToken,
    pub(crate) shutdown: CancellationToken,
    pub(crate) max_in_flight: usize,
    pub(crate) ping_interval: Duration,
    pub(crate) pong_timeout: Duration,
}

#[derive(Serialize)]
struct ErrorReply<'a> {
    #[serde(rename = "rid", skip_serializing_if = "Option::is_none")]
    rid: Option<u32>,
    error: &'a str,
}

#[derive(Serialize)]
struct SuccessReply<'a> {
    #[serde(rename = "rid", skip_serializing_if = "Option::is_none")]
    rid: Option<u32>,
    #[serde(with = "serde_bytes")]
    res: &'a [u8],
}

fn encode_prereserved<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(256);
    bytes.extend_from_slice(&[0; 4]);
    rmp_serde::encode::write_named(&mut bytes, value).ok()?;
    let payload_len = u32::try_from(bytes.len() - 4).ok()?;
    bytes[..4].copy_from_slice(&payload_len.to_be_bytes());
    Some(bytes)
}

fn error_message(rid: Option<u32>, code: &'static str) -> Option<WriterMsg> {
    encode_prereserved(&ErrorReply { rid, error: code }).map(|bytes| WriterMsg::Reply(bytes, None))
}

fn denied_body(code: &'static str) -> Option<Vec<u8>> {
    rmp_serde::to_vec_named(&DbResponse::Error {
        code: code.to_owned(),
        message: code.to_owned(),
    })
    .ok()
}

/// RAII supervision: aborts the writer if the loop future unwinds.
struct WriterTaskGuard(Option<JoinHandle<()>>);

impl WriterTaskGuard {
    async fn finish(&mut self) {
        let Some(mut task) = self.0.take() else {
            return;
        };
        if tokio::time::timeout(Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for WriterTaskGuard {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

struct CloseSubscriptions(Arc<SubscriptionRegistry>);

impl Drop for CloseSubscriptions {
    fn drop(&mut self) {
        self.0.close_all();
    }
}

/// Run the token request loop over an upgraded WebSocket stream.
pub(crate) async fn run_token_loop(ctx: TokenLoopContext, stream: WebSocketStream<UpgradedIo>) {
    let heartbeat = shamir_transport_ws::HeartbeatState::default();
    let (reader, writer) = stream.split();
    run_token_halves(
        ctx,
        TokenWsFrameReader::with_heartbeat(reader, heartbeat.clone()),
        TokenWsFrameWriter::with_heartbeat(writer, heartbeat),
    )
    .await;
}

async fn run_token_halves<R, W>(ctx: TokenLoopContext, mut reader: R, writer: W)
where
    R: FrameReader + 'static,
    W: FrameWriter + 'static,
{
    let grant = Arc::new(ctx.grant);
    let capacity = ctx.max_in_flight.max(1);
    let semaphore = Arc::new(Semaphore::new(capacity));
    let (sender, mut receiver) = mpsc::channel::<WriterMsg>(capacity);
    let subscriptions = Arc::new(SubscriptionRegistry::new());
    let _close_subscriptions = CloseSubscriptions(Arc::clone(&subscriptions));
    let push = Arc::new(MpscPushSink::new(sender.clone()));
    let connection = Arc::new(ConnectionServices {
        conn_id: 0,
        push: Some(push),
        extensions: Some(Arc::clone(&subscriptions) as Arc<dyn std::any::Any + Send + Sync>),
    });
    let close = CloseIntent::new();
    let shutdown = ctx.shutdown.clone();
    let dispatch_cancel = CancellationToken::new();
    let writer_close = close.clone();
    let heartbeat = writer.heartbeat_state();
    let ping_interval = ctx.ping_interval;
    let pong_timeout = ctx.pong_timeout;
    let writer_task = tokio::spawn(async move {
        let mut writer = writer;
        run_writer_task(
            &mut writer,
            &mut receiver,
            heartbeat,
            ping_interval,
            pong_timeout,
            writer_close,
        )
        .await;
    });
    let mut writer_task = WriterTaskGuard(Some(writer_task));
    let cleanup = Arc::new(TokenSessionTeardown::new(
        Arc::clone(&ctx.service),
        Arc::clone(&ctx.handler),
        ctx.session.session_id,
        Arc::clone(&subscriptions),
    ));
    cleanup.activate_metric();
    let scoped_handler = Arc::new(ScopedHandler::new(Arc::clone(&ctx.handler)));
    let mut dispatches = JoinSet::new();
    let deadline = tokio::time::Instant::now()
        + Duration::from_nanos(ctx.expires_at_ns.saturating_sub(unix_now_ns()));
    let mut revalidation = tokio::time::interval(Duration::from_secs(1));
    revalidation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut writer_done = false;
    'connection: loop {
        while let Some(result) = dispatches.try_join_next() {
            if let Err(error) = result {
                if error.is_panic() {
                    close.set_code(1011);
                    break 'connection;
                }
            }
        }
        reader.set_reader_parked(true);
        let permit = tokio::select! {
            biased;
            _ = shutdown.cancelled() => { close.set_code(1000); break 'connection; }
            _ = dispatch_cancel.cancelled() => break 'connection,
            _ = ctx.cancel.cancelled() => { close.set_code(4011); break 'connection; }
            _ = tokio::time::sleep_until(deadline) => { close.set_code(4012); break 'connection; }
            _ = revalidation.tick() => {
                let now = unix_now_ns();
                if !ctx.service.still_valid(&ctx.grant, now) {
                    close.set_code(if now >= ctx.expires_at_ns { 4012 } else { 4011 });
                    break 'connection;
                }
                continue;
            }
            permit = Arc::clone(&semaphore).acquire_owned() => match permit { Ok(permit) => permit, Err(_) => break 'connection },
            _ = writer_task.0.as_mut().expect("writer task active") => {
                writer_task.0.take();
                writer_done = true;
                break 'connection;
            },
        };

        reader.set_reader_parked(false);
        let mut frame = Vec::new();
        let read_result = {
            let read = reader.read_frame_into(16 * 1024 * 1024, &mut frame);
            tokio::pin!(read);
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => { close.set_code(1000); break 'connection; }
                    _ = dispatch_cancel.cancelled() => break 'connection,
                    _ = ctx.cancel.cancelled() => { close.set_code(4011); break 'connection; }
                    _ = tokio::time::sleep_until(deadline) => { close.set_code(4012); break 'connection; }
                    _ = revalidation.tick() => {
                        let now = unix_now_ns();
                        if !ctx.service.still_valid(&ctx.grant, now) {
                            close.set_code(if now >= ctx.expires_at_ns { 4012 } else { 4011 });
                            break 'connection;
                        }
                    }
                    result = &mut read => break result,
                    _ = writer_task.0.as_mut().expect("writer task active") => {
                        writer_task.0.take();
                        writer_done = true;
                        break 'connection;
                    },
                }
            }
        };
        if let Err(error) = read_result {
            close.set(&error);
            break 'connection;
        }

        // Keep the borrowed envelope entirely within this block; no read future
        // can retain the mutable frame borrow into dispatch parsing.
        let envelope = match RequestEnvelopeView::from_msgpack(&frame) {
            Ok(envelope) => envelope,
            Err(_) => {
                let Some(message) = error_message(None, "invalid_envelope") else {
                    break 'connection;
                };
                if sender.send(message).await.is_err() {
                    break 'connection;
                }
                drop(permit);
                continue;
            }
        };
        if envelope
            .session_id_array()
            .map(|sid| sid != &ctx.session.session_id)
            .unwrap_or(true)
        {
            close.set_code(4010);
            drop(permit);
            break 'connection;
        }
        let request_id = envelope.request_id;
        let request_bytes = envelope.req.to_vec();
        drop(envelope);
        let service = Arc::clone(&ctx.service);
        let grant = Arc::clone(&ctx.grant);
        let session = Arc::clone(&ctx.session);
        let scoped = Arc::clone(&scoped_handler);
        let connection = Arc::clone(&connection);
        let sender = sender.clone();
        let cancel = dispatch_cancel.clone();
        let close_dispatch = close.clone();
        let cleanup_lifetime = Arc::clone(&cleanup);
        let deadline_ns = ctx.expires_at_ns;
        dispatches.spawn(async move {
            let _cleanup_lifetime = cleanup_lifetime;
            let _permit = permit;
            let now = unix_now_ns();
            if now >= deadline_ns || !service.still_valid(&grant, now) {
                let code = if now >= deadline_ns { 4012 } else { 4011 };
                close_dispatch.set_code(code);
                cancel.cancel();
                return;
            }
            if session.check_post_auth_rate_limit(now).is_some() {
                let Some(message) = error_message(request_id, "rate_limited") else {
                    return;
                };
                let _ = sender.send(message).await;
                return;
            }
            if shamir_query_types::validate_request_alias_caps(&request_bytes).is_err() {
                let Some(message) = error_message(request_id, "invalid_request") else {
                    return;
                };
                let _ = sender.send(message).await;
                return;
            }
            let request: DbRequest = match rmp_serde::from_slice(&request_bytes) {
                Ok(request) => request,
                Err(_) => {
                    let Some(message) = error_message(request_id, "invalid_request") else {
                        return;
                    };
                    let _ = sender.send(message).await;
                    return;
                }
            };
            let now = unix_now_ns();
            if now >= deadline_ns || !service.still_valid(&grant, now) {
                let code = if now >= deadline_ns { 4012 } else { 4011 };
                close_dispatch.set_code(code);
                cancel.cancel();
                return;
            }
            let (result, budget) = byte_budget::run_with_handoff(scoped.handle(
                &session,
                request,
                Endpoint::Ws,
                &connection,
            ))
            .await;
            match result {
                Ok(response) => {
                    let response_bytes = match byte_budget::take_stashed_serialized_response() {
                        Some(bytes) => bytes,
                        None => response,
                    };
                    let message = match encode_prereserved(&SuccessReply {
                        rid: request_id,
                        res: &response_bytes,
                    }) {
                        Some(bytes) => WriterMsg::Reply(bytes, budget),
                        None => return,
                    };
                    let _ = sender.send(message).await;
                }
                Err(HandleDenial::Forbidden(code)) => {
                    let Some(response) = denied_body(code).and_then(|res| {
                        encode_prereserved(&SuccessReply {
                            rid: request_id,
                            res: &res,
                        })
                    }) else {
                        return;
                    };
                    if sender.send(WriterMsg::Reply(response, None)).await.is_err() {
                        return;
                    }
                }
                Err(HandleDenial::BadRequest(_)) => {
                    let Some(message) = error_message(request_id, "invalid_request") else {
                        return;
                    };
                    let _ = sender.send(message).await;
                    return;
                }
                Err(HandleDenial::ApplicationError) => {
                    let Some(message) = error_message(request_id, "internal_error") else {
                        return;
                    };
                    let _ = sender.send(message).await;
                    return;
                }
            }
        });
    }

    subscriptions.close_all();
    dispatches.abort_all();
    while dispatches.join_next().await.is_some() {}
    drop(connection);
    drop(sender);
    if !writer_done {
        writer_task.finish().await;
    }
    drop(cleanup);
}

fn unix_now_ns() -> u64 {
    shamir_connect::common::time::UnixNanos::try_now()
        .map(|now| now.as_u64())
        .unwrap_or(u64::MAX)
}
