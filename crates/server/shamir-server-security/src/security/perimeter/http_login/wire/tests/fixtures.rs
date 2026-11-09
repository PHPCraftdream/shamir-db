use super::super::{TokenScope, TokenScopeChannels, TokenScopeMode};

pub(super) struct BeginFields<'a> {
    pub(super) user: &'a str,
    pub(super) nonce: &'a [u8],
    pub(super) version: &'a [u8],
    pub(super) binding: &'a [u8],
    pub(super) token: &'a [u8],
}

pub(super) fn begin(fields: BeginFields<'_>) -> Vec<u8> {
    let mut out = vec![0x85];
    pair(&mut out, "protocol_version", fields.version);
    pair(&mut out, "user", &encoded_string(fields.user));
    let mut nonce = vec![
        0xc4,
        u8::try_from(fields.nonce.len()).expect("fixture nonce"),
    ];
    nonce.extend_from_slice(fields.nonce);
    pair(&mut out, "client_nonce", &nonce);
    pair(&mut out, "binding_mode", fields.binding);
    pair(&mut out, "token", fields.token);
    out
}

pub(super) fn valid_begin() -> Vec<u8> {
    let mut token = vec![0x84];
    pair(&mut token, "label", &encoded_string("http-login"));
    pair(&mut token, "mode", &encoded_string("read_only"));
    pair(&mut token, "channels", &encoded_string("rest"));
    pair(&mut token, "databases", &[0xc0]);
    begin(BeginFields {
        user: "alice",
        nonce: &[1; 32],
        version: &[1],
        binding: &[3],
        token: &token,
    })
}

pub(super) fn fields<'a>() -> BeginFields<'a> {
    BeginFields {
        user: "alice",
        nonce: &[1; 32],
        version: &[1],
        binding: &[3],
        token: &[
            0x84, 0xa5, b'l', b'a', b'b', b'e', b'l', 0xa1, b'x', 0xa4, b'm', b'o', b'd', b'e',
            0xa9, b'r', b'e', b'a', b'd', b'_', b'o', b'n', b'l', b'y', 0xa8, b'c', b'h', b'a',
            b'n', b'n', b'e', b'l', b's', 0xa4, b'r', b'e', b's', b't', 0xa9, b'd', b'a', b't',
            b'a', b'b', b'a', b's', b'e', b's', 0xc0,
        ],
    }
}

pub(super) fn encoded_string(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    if bytes.len() < 32 {
        out.push(0xa0 | bytes.len() as u8);
    } else if bytes.len() <= 255 {
        out.extend_from_slice(&[0xd9, bytes.len() as u8]);
    } else {
        out.push(0xda);
        out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(bytes);
    out
}

pub(super) fn pair(out: &mut Vec<u8>, name: &str, value: &[u8]) {
    out.extend_from_slice(&encoded_string(name));
    out.extend_from_slice(value);
}

pub(super) fn scope<'a>(label: &'a str, databases: Option<Vec<&'a str>>) -> TokenScope<'a> {
    TokenScope {
        label,
        mode: TokenScopeMode::ReadOnly,
        channels: TokenScopeChannels::Rest,
        databases,
    }
}
