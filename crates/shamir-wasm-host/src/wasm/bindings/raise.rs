use crate::runtime::error::FunctionError;
use wasmtime::{Caller, Error};

pub(crate) const MAX_GUEST_ERROR_MESSAGE_BYTES: usize = 1024;

fn push_bounded_char(message: &mut String, character: char) -> bool {
    if message.len() + character.len_utf8() > MAX_GUEST_ERROR_MESSAGE_BYTES {
        return false;
    }
    message.push(character);
    true
}

pub(crate) fn decode_guest_error(kind: i32, bytes: &[u8], declared_len: usize) -> FunctionError {
    let raw_len = bytes
        .len()
        .min(declared_len)
        .min(MAX_GUEST_ERROR_MESSAGE_BYTES);
    let mut message = String::with_capacity(MAX_GUEST_ERROR_MESSAGE_BYTES + " [truncated]".len());
    'chunks: for chunk in bytes[..raw_len].utf8_chunks() {
        for character in chunk.valid().chars() {
            if !push_bounded_char(
                &mut message,
                if character.is_control() {
                    '\u{fffd}'
                } else {
                    character
                },
            ) {
                break 'chunks;
            }
        }
        if !chunk.invalid().is_empty() && !push_bounded_char(&mut message, '\u{fffd}') {
            break;
        }
    }
    if declared_len > MAX_GUEST_ERROR_MESSAGE_BYTES {
        message.push_str(" [truncated]");
    }
    match kind {
        1 => FunctionError::User(message),
        2 => FunctionError::Compute(format!("guest invalid argument: {message}")),
        3 => FunctionError::Compute(format!("guest codec: {message}")),
        4 => FunctionError::Compute(format!("guest host: {message}")),
        0 | i32::MIN..=-1 | 5..=i32::MAX => {
            FunctionError::Compute(format!("unknown guest error kind {kind}: {message}"))
        }
    }
}

pub(crate) fn host_raise_error(
    mut caller: Caller<'_, super::super::wasm_function::HostState>,
    kind: i32,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<()> {
    if ptr < 0 || len < 0 {
        return Err(Error::new(FunctionError::Compute(
            "raise_error: negative pointer or length".to_string(),
        )));
    }
    let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
        return Err(Error::new(FunctionError::Compute(
            "raise_error: missing memory".to_string(),
        )));
    };
    let start = ptr as usize;
    let read_len = (len as usize).min(MAX_GUEST_ERROR_MESSAGE_BYTES);
    let Some(end) = start.checked_add(read_len) else {
        return Err(Error::new(FunctionError::Compute(
            "raise_error: pointer overflow".to_string(),
        )));
    };
    let data = memory.data(&caller);
    if end > data.len() {
        return Err(Error::new(FunctionError::Compute(
            "raise_error: guest memory out of bounds".to_string(),
        )));
    }
    Err(Error::new(decode_guest_error(
        kind,
        &data[start..end],
        len as usize,
    )))
}
