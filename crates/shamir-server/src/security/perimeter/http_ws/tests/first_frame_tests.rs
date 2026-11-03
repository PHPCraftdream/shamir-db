use std::time::Duration;

use super::super::browser::first_frame::{read_browser_token, FIRST_FRAME_MAX};
use tokio::io::AsyncWriteExt;

fn msgpack_token(length: usize) -> Vec<u8> {
    let mut payload = vec![0x81, 0xa5];
    payload.extend_from_slice(b"token");
    match length {
        0..=31 => payload.push(0xa0 | length as u8),
        32..=255 => payload.extend_from_slice(&[0xd9, length as u8]),
        _ => unreachable!(),
    }
    payload.extend(std::iter::repeat_n(b'x', length));
    payload
}

fn frame(opcode: u8, final_bit: bool, rsv: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(if final_bit { 0x80 } else { 0 }) | rsv | opcode];
    if payload.len() < 126 {
        out.push(0x80 | payload.len() as u8);
    } else {
        out.extend_from_slice(&[0x80 | 126, (payload.len() >> 8) as u8, payload.len() as u8]);
    }
    let mask = [3, 9, 17, 33];
    out.extend_from_slice(&mask);
    out.extend(
        payload
            .iter()
            .enumerate()
            .map(|(i, byte)| byte ^ mask[i % 4]),
    );
    out
}

async fn read_one(bytes: Vec<u8>) -> Result<String, ()> {
    let (mut writer, mut reader) = tokio::io::duplex(bytes.len() + 1);
    writer.write_all(&bytes).await.map_err(|_| ())?;
    read_browser_token(&mut reader).await
}

#[tokio::test]
async fn valid_80_byte_payload_is_accepted() {
    let payload = msgpack_token(71);
    assert_eq!(payload.len(), 80);
    let bytes = frame(2, true, 0, &payload);
    assert_eq!(bytes.len(), 86);
    assert_eq!(read_one(bytes).await.unwrap().len(), 71);
}

#[tokio::test(start_paused = true)]
async fn first_frame_size_boundary_is_checked_before_payload_read() {
    assert_eq!(FIRST_FRAME_MAX, 256);
    // A 256-byte payload header and mask are accepted; the read waits only for its payload.
    let accepted_header_and_mask = [0x82, 0xfe, 0x01, 0x00, 1, 2, 3, 4];
    let (mut writer, mut reader) = tokio::io::duplex(16);
    writer.write_all(&accepted_header_and_mask).await.unwrap();
    let mut read = Box::pin(read_browser_token(&mut reader));
    assert!(tokio::time::timeout(Duration::from_secs(1), &mut read)
        .await
        .is_err());
    drop(read);

    // A 257-byte header is rejected before waiting for a mask or payload.
    let rejected_header = [0x82, 0xfe, 0x01, 0x01];
    let (mut writer, mut reader) = tokio::io::duplex(16);
    writer.write_all(&rejected_header).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), read_browser_token(&mut reader)).await,
        Ok(Err(()))
    ));
}

#[tokio::test]
async fn invalid_initial_frame_matrix_is_rejected() {
    let payload = msgpack_token(71);
    for bytes in [
        frame(1, true, 0, &payload),
        frame(0, true, 0, &payload),
        frame(2, false, 0, &payload),
        frame(2, true, 0x40, &payload),
    ] {
        assert!(read_one(bytes).await.is_err());
    }
    let mut unmasked = vec![0x82, payload.len() as u8];
    unmasked.extend_from_slice(&payload);
    assert!(read_one(unmasked).await.is_err());
    assert!(read_one(frame(9, true, 0, &[0_u8; 126])).await.is_err());
}

#[tokio::test]
async fn controls_are_bounded_and_close_is_terminal() {
    let mut bytes = Vec::new();
    for _ in 0..8 {
        bytes.extend(frame(9, true, 0, &[]));
    }
    bytes.extend(frame(2, true, 0, &msgpack_token(71)));
    assert!(read_one(bytes).await.is_ok());
    let mut bytes = Vec::new();
    for _ in 0..9 {
        bytes.extend(frame(10, true, 0, &[]));
    }
    bytes.extend(frame(2, true, 0, &msgpack_token(71)));
    assert!(read_one(bytes).await.is_err());
    assert!(read_one(frame(8, true, 0, &[])).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn first_frame_deadline_is_absolute_despite_control_traffic() {
    let (mut writer, mut reader) = tokio::io::duplex(32);
    let task = tokio::spawn(async move { read_browser_token(&mut reader).await });
    for _ in 0..2 {
        writer.write_all(&frame(9, true, 0, &[])).await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
    }
    tokio::task::yield_now().await;
    assert!(
        task.is_finished(),
        "control frames must not extend the five-second absolute deadline"
    );
    assert!(task.await.unwrap().is_err());
}

#[tokio::test]
async fn exact_msgpack_shape_rejects_bad_keys_types_lengths_and_trailing_data() {
    let token = msgpack_token(71);
    assert!(read_one(frame(2, true, 0, &token)).await.is_ok());
    let mut trailing = token.clone();
    trailing.push(0);
    assert!(read_one(frame(2, true, 0, &trailing)).await.is_err());
    for length in [70, 72] {
        assert!(read_one(frame(2, true, 0, &msgpack_token(length)))
            .await
            .is_err());
    }
    let mut bin_value = vec![0x81, 0xa5];
    bin_value.extend_from_slice(b"token");
    bin_value.extend_from_slice(&[0xc4, 71]);
    bin_value.extend(std::iter::repeat_n(b'x', 71));
    assert!(read_one(frame(2, true, 0, &bin_value)).await.is_err());
    let mut bin_key = vec![0x81, 0xc4, 5];
    bin_key.extend_from_slice(b"token");
    bin_key.extend_from_slice(&msgpack_token(71)[7..]);
    assert!(read_one(frame(2, true, 0, &bin_key)).await.is_err());
    let mut repeated = vec![0x82];
    repeated.extend_from_slice(&token[1..]);
    repeated.extend_from_slice(&token[1..]);
    assert!(read_one(frame(2, true, 0, &repeated)).await.is_err());
    let missing = [0x80];
    assert!(read_one(frame(2, true, 0, &missing)).await.is_err());
    let non_string = [0x81, 0xa5, b't', b'o', b'k', b'e', b'n', 0xc0];
    assert!(read_one(frame(2, true, 0, &non_string)).await.is_err());

    let mut unknown_key = vec![0x81, 0xa5];
    unknown_key.extend_from_slice(b"other");
    unknown_key.extend_from_slice(&token[7..]);
    assert!(read_one(frame(2, true, 0, &unknown_key)).await.is_err());

    for marker in [0xda, 0xdb] {
        let mut wide_string = vec![0x81, 0xa5];
        wide_string.extend_from_slice(b"token");
        if marker == 0xda {
            wide_string.extend_from_slice(&[marker, 0, 71]);
        } else {
            wide_string.extend_from_slice(&[marker, 0, 0, 0, 71]);
        }
        wide_string.extend(std::iter::repeat_n(b'x', 71));
        assert!(read_one(frame(2, true, 0, &wide_string)).await.is_ok());
    }
}
