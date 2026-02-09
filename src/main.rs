#![allow(deprecated)]

use shamir_db::codecs::Codec;
use shamir_db::codecs::message_pack::MessagePackCodec;
use shamir_db::types::value::UserValue;

fn main() {
    println!("S.H.A.M.I.R. Database");

    let codec = MessagePackCodec;
    let value = UserValue::Str("Hello, SHAMIR!".to_string());
    let encoded = codec.encode(&value).unwrap();
    let decoded: UserValue = codec.decode(&encoded).unwrap();

    println!("Original: {:?}", value);
    println!("Encoded: {:?}", encoded);
    println!("Decoded: {:?}", decoded);
}
