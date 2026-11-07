/// How a [`Client`](crate::Client) session was authenticated.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthKind {
    /// SCRAM-authenticated session, including resumed sessions.
    Scram,
    /// API-token session over the token WebSocket.
    Token,
}
