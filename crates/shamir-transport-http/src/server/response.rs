use bytes::Bytes;
use http::{header, HeaderValue, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full};

pub(crate) fn response(
    status: StatusCode,
    body: Bytes,
    tls: bool,
    hsts: bool,
    close: bool,
) -> Response<BoxBody<Bytes, std::convert::Infallible>> {
    response_held(
        status,
        Full::new(body).map_err(|never| match never {}).boxed(),
        tls,
        hsts,
        close,
    )
}

pub(crate) fn response_held(
    status: StatusCode,
    body: BoxBody<Bytes, std::convert::Infallible>,
    tls: bool,
    hsts: bool,
    close: bool,
) -> Response<BoxBody<Bytes, std::convert::Infallible>> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/msgpack"),
    );
    if tls && hsts {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    if close {
        headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
    response
}

pub(crate) fn canned(
    status: StatusCode,
    tls: bool,
    hsts: bool,
) -> Response<BoxBody<Bytes, std::convert::Infallible>> {
    response(status, Bytes::new(), tls, hsts, true)
}
