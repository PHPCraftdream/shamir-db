//! Token WebSocket session binding, framing, loop, and teardown.

mod close_guard;
mod framer;
mod teardown;
#[cfg(test)]
mod tests;
mod token_loop;

pub use crate::security::http_ws::TokenLoopContext;
pub(crate) use close_guard::TokenWsCloseGuard;
pub(crate) use framer::{TokenWsFrameReader, TokenWsFrameWriter};
pub(crate) use teardown::TokenSessionTeardown;
pub use token_loop::{
    encode_prereserved, linger_after_close, run_token_halves, run_token_loop, ErrorReply,
};
