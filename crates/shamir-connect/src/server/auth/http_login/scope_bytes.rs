/// Access mode encoded in HTTP-login scope bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpLoginMode {
    /// Grant read operations only.
    ReadOnly,
    /// Grant read and write operations.
    ReadWrite,
}

/// Channel set encoded in HTTP-login scope bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpLoginChannels {
    /// Permit REST requests.
    Rest,
    /// Permit WebSocket requests.
    Ws,
    /// Permit REST and WebSocket requests.
    Both,
}

/// Token scope frozen into the HTTP-login proof transcript.
pub struct HttpLoginScope<'a> {
    /// UTF-8 label bytes (1..=128), with no C0 or DEL controls.
    pub label: &'a str,
    /// Requested authorization mode.
    pub mode: HttpLoginMode,
    /// Requested network channels.
    pub channels: HttpLoginChannels,
    /// `None` means all databases; otherwise a non-empty sorted allowlist.
    pub databases: Option<&'a [&'a str]>,
}

/// Encode the protocol's bounded scope representation with a preflight size check.
pub fn encode_scope(scope: &HttpLoginScope<'_>) -> Result<Vec<u8>, &'static str> {
    let label = scope.label.as_bytes();
    if label.is_empty()
        || label.len() > 128
        || label.iter().any(|byte| *byte < 0x20 || *byte == 0x7f)
    {
        return Err("invalid label");
    }
    let mut encoded_len = 5usize.checked_add(label.len()).ok_or("scope too long")?;
    if let Some(databases) = scope.databases {
        if databases.is_empty() || databases.len() > 4 {
            return Err("invalid database count");
        }
        let mut previous: Option<&[u8]> = None;
        for database in databases {
            let bytes = database.as_bytes();
            if bytes.is_empty()
                || bytes.len() > 255
                || !bytes
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
            {
                return Err("invalid database name");
            }
            if previous.is_some_and(|prior| prior >= bytes) {
                return Err("databases must be strictly sorted");
            }
            previous = Some(bytes);
            encoded_len = encoded_len
                .checked_add(2)
                .and_then(|len| len.checked_add(bytes.len()))
                .ok_or("scope too long")?;
        }
    }
    if encoded_len > 512 {
        return Err("scope too long");
    }
    let mut out = Vec::with_capacity(encoded_len);
    out.extend_from_slice(
        &u16::try_from(label.len())
            .map_err(|_| "label too long")?
            .to_be_bytes(),
    );
    out.extend_from_slice(label);
    out.push(match scope.mode {
        HttpLoginMode::ReadOnly => 0,
        HttpLoginMode::ReadWrite => 1,
    });
    out.push(match scope.channels {
        HttpLoginChannels::Rest => 0,
        HttpLoginChannels::Ws => 1,
        HttpLoginChannels::Both => 2,
    });
    match scope.databases {
        None => out.push(0xff),
        Some(databases) => {
            out.push(u8::try_from(databases.len()).map_err(|_| "invalid database count")?);
            for database in databases {
                let bytes = database.as_bytes();
                out.extend_from_slice(
                    &u16::try_from(bytes.len())
                        .map_err(|_| "database too long")?
                        .to_be_bytes(),
                );
                out.extend_from_slice(bytes);
            }
        }
    }
    Ok(out)
}
