use crate::value::Value;

const MAX_NESTING: usize = 32;

fn nested_arrays(count: usize) -> Vec<u8> {
    let mut bytes = vec![0x91; count];
    bytes.push(0xc0);
    bytes
}

fn nested_maps(count: usize) -> Vec<u8> {
    let mut bytes = vec![0x81, 0xa1, b'x'];
    for _ in 1..count {
        bytes.extend_from_slice(&[0x81, 0xa1, b'x']);
    }
    bytes.push(0xc0);
    bytes
}

fn mixed_nesting(count: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for index in 0..count {
        if index % 2 == 0 {
            bytes.extend_from_slice(&[0x81, 0xa1, b'x']);
        } else {
            bytes.push(0x91);
        }
    }
    bytes.push(0xc0);
    bytes
}

fn entrypoint_params(count: usize) -> Vec<u8> {
    let mut bytes = vec![0x81, 0xa1, b'p'];
    bytes.extend(nested_maps(count));
    bytes
}

#[test]
fn rv_sdk_07a35f13_accepts_limit_and_rejects_over_limit() {
    for encode in [
        nested_maps as fn(usize) -> Vec<u8>,
        nested_arrays,
        mixed_nesting,
    ] {
        for count in [MAX_NESTING, MAX_NESTING + 1] {
            let encoded = encode(count);
            let result = rmp_serde::from_slice::<Value>(&encoded);
            if count == MAX_NESTING {
                assert!(result.is_ok());
            } else {
                let error = result.expect_err("over-limit nesting must be rejected");
                assert!(error.to_string().contains("maximum Value nesting depth"));
            }
        }
    }
}

#[test]
fn rv_sdk_07a35f13_decode_params_enforces_nesting_limit() {
    let at_limit = entrypoint_params(MAX_NESTING - 1);
    assert!(crate::__rt::decode_params(&at_limit).is_ok());

    let over_limit = entrypoint_params(MAX_NESTING);
    let error = crate::__rt::decode_params(&over_limit)
        .expect_err("entrypoint params beyond the Value limit must be rejected");
    assert!(error.to_string().contains("maximum Value nesting depth"));
}
