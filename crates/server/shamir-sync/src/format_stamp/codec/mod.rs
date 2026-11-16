//! Stamp frame codec (design 2.3).

pub mod decode;
pub mod encode;
pub mod header;

pub use decode::decode;
pub use encode::encode;
pub use header::{
    read_header, HEADER_LEN, MAGIC, MAX_BODY_LEN, STAMP_FILE_NAME, STAMP_LAYOUT_VERSION,
    STAMP_TMP_NAME, TRAILER_LEN,
};
