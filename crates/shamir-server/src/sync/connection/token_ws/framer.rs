//! Raw-msgpack WebSocket frame reader and writer for token sessions.

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::SinkExt;
use shamir_transport_ws::framing::{
    ws_recv_raw_into_stream_observing, ws_send_sink_raw, HeartbeatState, WsFrameError,
    MAX_WS_FRAME_SIZE,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::error::CapacityError;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error, Message};
use tokio_tungstenite::WebSocketStream;

use crate::framer::{FrameReader, FrameWriter, FramerError};

/// Reader half that treats each binary WebSocket message as one msgpack frame.
pub(crate) struct TokenWsFrameReader<S> {
    stream: SplitStream<WebSocketStream<S>>,
    heartbeat: HeartbeatState,
}

/// Writer half that strips the shared request loop's private length prefix.
pub(crate) struct TokenWsFrameWriter<S> {
    stream: SplitSink<WebSocketStream<S>, Message>,
    heartbeat: HeartbeatState,
}

impl<S> TokenWsFrameReader<S> {
    /// Build a reader sharing heartbeat state with the writer half.
    pub(crate) fn with_heartbeat(
        stream: SplitStream<WebSocketStream<S>>,
        heartbeat: HeartbeatState,
    ) -> Self {
        Self { stream, heartbeat }
    }
}

impl<S> TokenWsFrameWriter<S> {
    /// Build a writer sharing heartbeat state with the reader half.
    pub(crate) fn with_heartbeat(
        stream: SplitSink<WebSocketStream<S>, Message>,
        heartbeat: HeartbeatState,
    ) -> Self {
        Self { stream, heartbeat }
    }
}

impl<S> FrameReader for TokenWsFrameReader<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn read_frame_into(&mut self, max: usize, buf: &mut Vec<u8>) -> Result<(), FramerError> {
        ws_recv_raw_into_stream_observing(
            &mut self.stream,
            max.min(MAX_WS_FRAME_SIZE),
            buf,
            &self.heartbeat,
        )
        .await
        .map_err(map_ws_err)
    }

    fn set_reader_parked(&mut self, parked: bool) {
        self.heartbeat.reader_parked(parked);
    }
}

impl<S> FrameWriter for TokenWsFrameWriter<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn write_frame_into(
        &mut self,
        payload: &[u8],
        _: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        ws_send_sink_raw(&mut self.stream, payload)
            .await
            .map_err(map_ws_err)
    }

    async fn write_frame_prereserved(&mut self, buf: Vec<u8>) -> Result<(), FramerError> {
        if buf.len() < 4 {
            return Err(FramerError::Decode("short length prefix".into()));
        }
        let declared = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if declared != buf.len() - 4 {
            return Err(FramerError::Decode("length prefix mismatch".into()));
        }
        ws_send_sink_raw(&mut self.stream, &buf[4..])
            .await
            .map_err(map_ws_err)
    }

    fn heartbeat_state(&self) -> Option<HeartbeatState> {
        Some(self.heartbeat.clone())
    }

    async fn send_heartbeat(&mut self, payload: [u8; 8]) -> Result<(), FramerError> {
        self.stream
            .send(Message::Ping(payload.to_vec().into()))
            .await
            .map_err(|error| FramerError::Io(error.to_string()))
    }

    async fn close_with(&mut self, code: u16, reason: &str) {
        let _ = self
            .stream
            .send(Message::Close(Some(CloseFrame {
                code: code.into(),
                reason: reason.into(),
            })))
            .await;
        let _ = self.stream.close().await;
    }

    async fn shutdown(&mut self) {
        self.close_with(1000, "normal closure").await;
    }
}

fn map_ws_err(error: WsFrameError) -> FramerError {
    match error {
        WsFrameError::PeerClose => FramerError::PeerClose,
        WsFrameError::TooLarge { actual, max } => FramerError::TooLarge { actual, max },
        WsFrameError::NonBinaryMessage(_) => {
            FramerError::UnsupportedData("non-binary message".into())
        }
        WsFrameError::LengthMismatch { .. } => FramerError::Decode("length mismatch".into()),
        WsFrameError::ControlFlood { .. } => FramerError::Decode("control flood".into()),
        WsFrameError::Io(error) => match error {
            Error::Capacity(CapacityError::MessageTooLong { size, max_size }) => {
                FramerError::TooLarge {
                    actual: size,
                    max: max_size,
                }
            }
            Error::Protocol(_) => FramerError::Decode(error.to_string()),
            other => FramerError::Io(other.to_string()),
        },
    }
}
