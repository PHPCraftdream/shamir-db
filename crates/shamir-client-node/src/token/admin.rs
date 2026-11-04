use napi::bindgen_prelude::*;
use napi_derive::napi;

use shamir_client as core;

use super::views::CreatedTokenView;
use crate::client::ShamirClient;
use crate::{encode_buffer, encode_db_error, infra_error, run_request};

/// Parameters of `ShamirClient.createApiToken`.
#[napi(object)]
pub struct CreateApiTokenOptions {
    /// Token label.
    pub label: String,
    /// Lifetime in seconds; the server default applies when omitted.
    pub ttl_secs: Option<u32>,
    /// `"read_only"` or `"read_write"`.
    pub mode: Option<String>,
    /// `"rest"`, `"ws"` or `"both"`.
    pub channels: Option<String>,
    /// Database allowlist; omitted means every accessible database.
    pub databases: Option<Vec<String>>,
    /// Mint the token for this user instead of the session user.
    pub for_user: Option<String>,
}

fn parse_mode(mode: &str) -> Result<core::ApiTokenMode> {
    match mode {
        "read_only" => Ok(core::ApiTokenMode::ReadOnly),
        "read_write" => Ok(core::ApiTokenMode::ReadWrite),
        _ => Err(Error::from_reason("invalid mode")),
    }
}

fn parse_channels(channels: &str) -> Result<core::ApiTokenChannels> {
    match channels {
        "rest" => Ok(core::ApiTokenChannels::Rest),
        "ws" => Ok(core::ApiTokenChannels::Ws),
        "both" => Ok(core::ApiTokenChannels::Both),
        _ => Err(Error::from_reason("invalid channels")),
    }
}

/// `ShamirClient.createApiToken`: success is the named msgpack of
/// [`CreatedTokenView`], a DB error is the usual error-marker `Buffer`.
pub(crate) fn create_api_token<'env>(
    client: &ShamirClient,
    env: &'env Env,
    opts: CreateApiTokenOptions,
) -> Result<PromiseRaw<'env, Buffer>> {
    let (state, mut shutdown, deadline, timeout) = client.request_parts();
    let mode = opts.mode.as_deref().map(parse_mode).transpose()?;
    let channels = opts.channels.as_deref().map(parse_channels).transpose()?;
    let mut request = core::builder::token_admin::create_api_token(opts.label);
    if let Some(ttl_secs) = opts.ttl_secs {
        request = request.ttl_secs(u64::from(ttl_secs));
    }
    if let Some(mode) = mode {
        request = request.mode(mode);
    }
    if let Some(channels) = channels {
        request = request.channels(channels);
    }
    if let Some(databases) = opts.databases {
        request = request.databases(databases);
    }
    if let Some(user) = opts.for_user {
        request = request.for_user(user);
    }
    env.spawn_future(async move {
        let result = run_request(&state, &mut shutdown, deadline, timeout, |client| {
            Box::pin(async move { client.create_api_token(request).await })
        })
        .await?;
        match result {
            Ok(created) => encode_buffer(
                &CreatedTokenView::from_created(created),
                "encode created token",
            ),
            Err(core::ClientError::Db { code, message }) => encode_db_error(code, message),
            Err(error) => Err(infra_error(error)),
        }
    })
}
