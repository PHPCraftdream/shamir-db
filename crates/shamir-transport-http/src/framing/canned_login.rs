pub(crate) fn canned_error_body(status: u16) -> &'static [u8] {
    match status {
        408 => b"\x81\xa5error\xafrequest_timeout",
        413 => b"\x81\xa5error\xb1request_too_large",
        431 => b"\x81\xa5error\xb9request_headers_too_large",
        _ => b"\x81\xa5error\xafinvalid_request",
    }
}
