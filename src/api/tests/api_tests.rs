#![allow(deprecated)]

use crate::api::{Command, Request, Response};
use crate::codecs::basic::MessagePackCodec;
use crate::codecs::Codec;
use crate::types::common::new_map;
use crate::types::value::UserValue;

#[test]
fn test_request_response_roundtrip() {
    let codec = MessagePackCodec;

    // 1. Create a complex Put command
    let mut map = new_map();
    map.insert("nested_key".to_string(), UserValue::Int(100));
    let put_command = Command::Put {
        key: "test_key".to_string(),
        value: UserValue::Map(map),
    };
    let request = Request {
        request_id: 1,
        command: put_command,
    };

    // Test Request round-trip
    let encoded_req = codec.encode(&request).unwrap();
    let decoded_req: Request = codec.decode(&encoded_req).unwrap();
    assert_eq!(request, decoded_req);

    // 2. Create a Get command
    let get_command = Command::Get {
        key: "test_key".to_string(),
    };
    let request = Request {
        request_id: 2,
        command: get_command,
    };
    let encoded_req = codec.encode(&request).unwrap();
    let decoded_req: Request = codec.decode(&encoded_req).unwrap();
    assert_eq!(request, decoded_req);

    // 3. Create a successful Response
    let response_ok = Response {
        request_id: 1,
        result: Ok(Some(UserValue::Str("Success".to_string()))),
    };
    let encoded_res = codec.encode(&response_ok).unwrap();
    let decoded_res: Response = codec.decode(&encoded_res).unwrap();
    assert_eq!(response_ok, decoded_res);

    // 4. Create an error Response
    let response_err = Response {
        request_id: 2,
        result: Err("Key not found".to_string()),
    };
    let encoded_res = codec.encode(&response_err).unwrap();
    let decoded_res: Response = codec.decode(&encoded_res).unwrap();
    assert_eq!(response_err, decoded_res);
}
