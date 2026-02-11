#![allow(deprecated)]

use crate::types::value::UserValue;
use serde::{Deserialize, Serialize};

/// Represents all possible operations that can be performed on the database.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum Command {
    /// Inserts or updates a value for a given key.
    Put { key: String, value: UserValue },

    /// Retrieves a value for a given key.
    Get { key: String },

    /// Deletes a key-value pair.
    Del { key: String },

    /// Executes a WASM function. (For future use)
    Execute { func: String, args: Vec<UserValue> },
}

/// A request sent from a client to the server.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Request {
    /// A unique ID to correlate requests with responses.
    pub request_id: u64,
    /// The command to be executed.
    pub command: Command,
}

/// A response sent from the server back to the client.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Response {
    /// The ID of the request this response corresponds to.
    pub request_id: u64,
    /// The result of the operation.
    /// Ok(Some(value)) for successful Get.
    /// Ok(None) for successful Put, Del, or Get on a non-existent key.
    /// Err(message) for any failure.
    pub result: Result<Option<UserValue>, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::basic::MessagePackCodec;
    use crate::codecs::Codec;
    use crate::types::common::new_map;

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
}
