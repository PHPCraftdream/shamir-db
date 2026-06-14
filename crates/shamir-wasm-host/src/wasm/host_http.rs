use super::super::net_gateway::{HttpRequest, HttpResponse};
use super::wasm_function::{read_guest_mem, write_value_to_guest, HostState};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

// ── Async host import: http_fetch (slice 8c) ─────────────────────────
//
// Same three-phase borrow dance as host_db_get:
// 1. Read request msgpack bytes from guest memory.
// 2. Clone Arc<dyn NetGateway> from caller.data().
// 3. Drop Caller borrows, await the gateway method.
// 4. Re-acquire Caller, alloc + write result.

/// Decode a msgpack `Value::Map` into an [`HttpRequest`].
///
/// Expected wire shape:
/// ```text
/// { "method": Str, "url": Str, "headers": Map|List, "body": Bin }
/// ```
pub(super) fn decode_http_request(val: &QueryValue) -> Result<HttpRequest, String> {
    let map = match val {
        QueryValue::Map(entries) => entries,
        _ => return Err("http_fetch: request must be a Map".to_string()),
    };

    let get_str = |key: &str| -> Result<String, String> {
        map.get(key)
            .map(|v| match v {
                QueryValue::Str(s) => Ok(s.clone()),
                _ => Err(format!("http_fetch: {key} must be a string")),
            })
            .transpose()
            .map(|o| o.unwrap_or_default())
    };

    let method = get_str("method")?;
    let url = get_str("url")?;

    let headers = match map.get("headers") {
        Some(QueryValue::Map(entries)) => entries
            .iter()
            .map(|(k, v)| match v {
                QueryValue::Str(s) => Ok((k.clone(), s.clone())),
                _ => Err("http_fetch: header values must be strings".to_string()),
            })
            .collect::<Result<Vec<_>, String>>()?,
        Some(QueryValue::List(items)) => items
            .iter()
            .map(|item| match item {
                QueryValue::List(pair) if pair.len() == 2 => {
                    let k = match &pair[0] {
                        QueryValue::Str(s) => s.clone(),
                        _ => return Err("http_fetch: header name must be string".to_string()),
                    };
                    let v = match &pair[1] {
                        QueryValue::Str(s) => s.clone(),
                        _ => return Err("http_fetch: header value must be string".to_string()),
                    };
                    Ok((k, v))
                }
                _ => Err("http_fetch: header items must be [name, value] pairs".to_string()),
            })
            .collect::<Result<Vec<_>, String>>()?,
        _ => Vec::new(),
    };

    let body = match map.get("body") {
        Some(QueryValue::Bin(b)) => b.clone(),
        _ => Vec::new(),
    };

    Ok(HttpRequest {
        method,
        url,
        headers,
        body,
    })
}

/// Encode an [`HttpResponse`] into a msgpack `Value::Map`.
///
/// Wire shape:
/// ```text
/// { "status": Int, "headers": Map, "body": Bin }
/// ```
pub(super) fn encode_http_response(resp: HttpResponse) -> QueryValue {
    let mut header_map = new_map_wc(resp.headers.len());
    for (k, v) in resp.headers {
        header_map.insert(k, QueryValue::Str(v));
    }

    let mut map = new_map_wc(3);
    map.insert("status".to_string(), QueryValue::Int(resp.status as i64));
    map.insert("headers".to_string(), QueryValue::Map(header_map));
    map.insert("body".to_string(), QueryValue::Bin(resp.body));
    QueryValue::Map(map)
}

/// Host implementation of `http_fetch(req_ptr, req_len) -> i64`.
///
/// Decodes a msgpack `HttpRequest` value, calls the network gateway,
/// and returns the packed pointer to a msgpack envelope:
///
/// ```text
/// Value::List([ Value::Bool(ok), payload ])
/// ```
///
/// On `ok = true`, payload is the [`HttpResponse`] map.
/// On `ok = false`, payload is `Value::Str(error_message)`.
///
/// Only "no net gateway configured" traps (config bug, like db/ctx.call).
/// All runtime errors (allowlist denial, curl failure, timeout) are
/// returned as catchable `Err` via the envelope.
pub(super) fn host_http_fetch(
    mut caller: wasmtime::Caller<'_, HostState>,
    (req_ptr, req_len): (i32, i32),
) -> Box<dyn std::future::Future<Output = Result<i64, wasmtime::Error>> + Send + '_> {
    Box::new(async move {
        // Phase 1: read inputs (sync).
        let req_bytes;
        let net;
        {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .ok_or_else(|| wasmtime::Error::msg("http_fetch: missing export `memory`"))?;

            req_bytes = read_guest_mem(memory.data(&caller), req_ptr, req_len)?;

            net = caller.data().net.clone();
        }

        let req_val = QueryValue::from_bytes(&req_bytes)
            .map_err(|e| wasmtime::Error::msg(format!("http_fetch: request decode error: {e}")))?;
        let http_req = decode_http_request(&req_val).map_err(wasmtime::Error::msg)?;

        // "No net gateway" is a config bug → trap (not catchable).
        let gateway = net.ok_or_else(|| {
            wasmtime::Error::msg(
                "http_fetch: no net gateway (egress not configured for this invocation)",
            )
        })?;

        // Phase 2: await the gateway. Runtime errors are catchable.
        let envelope = match gateway.fetch(http_req).await {
            Ok(http_resp) => QueryValue::List(vec![
                QueryValue::Bool(true),
                encode_http_response(http_resp),
            ]),
            Err(msg) => QueryValue::List(vec![QueryValue::Bool(false), QueryValue::Str(msg)]),
        };

        // Phase 3: encode and write result back.
        write_value_to_guest(&mut caller, Some(envelope)).await
    })
}
