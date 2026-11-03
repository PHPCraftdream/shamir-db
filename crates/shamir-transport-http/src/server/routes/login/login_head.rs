use http::HeaderMap;

/// Borrowed, transport-owned view of a login request head.
pub struct LoginHead<'a> {
    method: &'a [u8],
    path: &'a [u8],
    headers: &'a HeaderMap,
}

impl<'a> LoginHead<'a> {
    pub(crate) fn new(method: &'a [u8], path: &'a [u8], headers: &'a HeaderMap) -> Self {
        Self {
            method,
            path,
            headers,
        }
    }

    /// Request method bytes, as received by the HTTP parser.
    pub fn method(&self) -> &'a [u8] {
        self.method
    }

    /// Request path bytes, excluding the query string.
    pub fn path(&self) -> &'a [u8] {
        self.path
    }

    /// All values for a header name, retaining duplicates and raw bytes.
    pub fn header_values<'b>(&'b self, name: &'b [u8]) -> impl Iterator<Item = &'b [u8]> + 'b {
        self.headers.iter().filter_map(move |(header_name, value)| {
            header_name
                .as_str()
                .as_bytes()
                .eq_ignore_ascii_case(name)
                .then_some(value.as_bytes())
        })
    }
}
