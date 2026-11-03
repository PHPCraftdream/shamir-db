use crate::common::kdf_params::KdfParams;
use crate::common::username::NormalizedUsername;

/// Fixed fields that bind an HTTP-login challenge to its proof.
pub struct HttpLoginTranscriptInput<'a> {
    /// Protocol version; currently 1.
    pub protocol_version: u8,
    /// Protocol-local binding mode; currently 3.
    pub binding_mode: u8,
    /// Listener security class (0, 1, or 2).
    pub security: u8,
    /// Stable logical listener ID.
    pub listener_id: &'a [u8; 16],
    /// Random pending-handshake ID.
    pub handshake_id: &'a [u8; 32],
    /// Canonical username UTF-8.
    pub user: &'a str,
    /// Client nonce.
    pub client_nonce: &'a [u8; 32],
    /// Server nonce.
    pub server_nonce: &'a [u8; 32],
    /// Verifier salt.
    pub salt: &'a [u8; 16],
    /// Argon2 memory in KiB.
    pub memory_kb: u32,
    /// Argon2 iteration count.
    pub time: u32,
    /// Argon2 parallelism.
    pub parallelism: u32,
    /// Argon2 version (0x13).
    pub argon2_version: u8,
    /// Scope encoded by [`super::encode_scope`].
    pub scope_bytes: &'a [u8],
}

/// Build the exact protocol-local transcript, rejecting unsupported values and bounds.
pub fn build_transcript(input: &HttpLoginTranscriptInput<'_>) -> Result<Vec<u8>, &'static str> {
    let user = input.user.as_bytes();
    if input.protocol_version != 1 {
        return Err("unsupported protocol version");
    }
    if input.binding_mode != 3 {
        return Err("unsupported binding mode");
    }
    if input.security > 2 {
        return Err("unsupported security class");
    }
    if !(1..=255).contains(&user.len()) {
        return Err("invalid username length");
    }
    let normalized = NormalizedUsername::from_raw(input.user).map_err(|_| "invalid username")?;
    if normalized.as_str().as_bytes() != user {
        return Err("username is not canonical");
    }
    if input.argon2_version != 0x13 {
        return Err("unsupported Argon2 version");
    }
    let params = KdfParams {
        memory_kb: input.memory_kb,
        time: input.time,
        parallelism: input.parallelism,
        argon2_version: input.argon2_version,
    };
    params
        .validate_client_limits()
        .map_err(|_| "KDF parameters outside client bounds")?;
    validate_scope_bytes(input.scope_bytes)?;
    if input.client_nonce.iter().all(|byte| *byte == 0) {
        return Err("zero client nonce");
    }
    if input.server_nonce.iter().all(|byte| *byte == 0) || input.server_nonce == input.client_nonce
    {
        return Err("invalid server nonce");
    }
    let total = 166usize
        .checked_add(user.len())
        .and_then(|len| len.checked_add(input.scope_bytes.len()))
        .ok_or("transcript too long")?;
    if !(173..=933).contains(&total) {
        return Err("transcript length out of range");
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"SHAMIR-HTTP-LOGIN-v1");
    out.extend_from_slice(&[input.protocol_version, input.binding_mode, input.security]);
    out.extend_from_slice(input.listener_id);
    out.extend_from_slice(input.handshake_id);
    out.extend_from_slice(
        &u16::try_from(user.len())
            .map_err(|_| "user too long")?
            .to_be_bytes(),
    );
    out.extend_from_slice(user);
    out.extend_from_slice(input.client_nonce);
    out.extend_from_slice(input.server_nonce);
    out.extend_from_slice(input.salt);
    out.extend_from_slice(&input.memory_kb.to_be_bytes());
    out.extend_from_slice(&input.time.to_be_bytes());
    out.extend_from_slice(&input.parallelism.to_be_bytes());
    out.push(input.argon2_version);
    out.extend_from_slice(input.scope_bytes);
    Ok(out)
}

fn validate_scope_bytes(scope: &[u8]) -> Result<(), &'static str> {
    if !(6..=512).contains(&scope.len()) {
        return Err("scope length out of range");
    }
    let label_len = usize::from(u16::from_be_bytes([scope[0], scope[1]]));
    let label_after = 2usize.checked_add(label_len).ok_or("scope too long")?;
    let fields_end = label_after.checked_add(3).ok_or("scope too long")?;
    if !(1..=128).contains(&label_len) || fields_end > scope.len() {
        return Err("invalid scope label length");
    }
    let label_end = 2 + label_len;
    if scope[2..label_end]
        .iter()
        .any(|byte| *byte < 0x20 || *byte == 0x7f)
    {
        return Err("invalid scope label");
    }
    std::str::from_utf8(&scope[2..label_end]).map_err(|_| "invalid scope label")?;
    let mode = scope[label_end];
    let channel = scope[label_end + 1];
    if mode > 1 || channel > 2 {
        return Err("invalid scope mode or channel");
    }
    let count = scope[label_end + 2];
    let mut offset = label_end + 3;
    if count == 0xff {
        return if offset == scope.len() {
            Ok(())
        } else {
            Err("trailing scope bytes")
        };
    }
    if !(1..=4).contains(&count) {
        return Err("invalid database count");
    }
    let mut previous: Option<&[u8]> = None;
    for _ in 0..count {
        let end_len = offset.checked_add(2).ok_or("scope too long")?;
        if end_len > scope.len() {
            return Err("truncated database length");
        }
        let len = usize::from(u16::from_be_bytes([scope[offset], scope[offset + 1]]));
        offset = end_len;
        let end = offset.checked_add(len).ok_or("scope too long")?;
        if len == 0 || len > 255 || end > scope.len() {
            return Err("invalid database length");
        }
        let bytes = &scope[offset..end];
        if !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
        {
            return Err("invalid database name");
        }
        if previous.is_some_and(|prior| prior >= bytes) {
            return Err("databases must be strictly sorted");
        }
        previous = Some(bytes);
        offset = end;
    }
    if offset != scope.len() {
        return Err("trailing scope bytes");
    }
    Ok(())
}
