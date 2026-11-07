use super::{validate_payload_len, FrameError};

#[test]
fn capped_writer_length_boundaries_without_allocation() {
    assert!(matches!(
        validate_payload_len(0, 0),
        Err(FrameError::EmptyPayload)
    ));
    assert!(validate_payload_len(8, 8).is_ok());
    assert!(matches!(
        validate_payload_len(9, 8),
        Err(FrameError::TooLarge { actual: 9, max: 8 })
    ));
    let max = (u32::MAX as usize).min(usize::MAX - 4);
    assert!(validate_payload_len(max, usize::MAX).is_ok());
    assert!(matches!(
        validate_payload_len(max + 1, usize::MAX),
        Err(FrameError::TooLarge { .. })
    ));
    #[cfg(target_pointer_width = "64")]
    {
        assert!(validate_payload_len(u32::MAX as usize, usize::MAX).is_ok());
        assert!(matches!(
            validate_payload_len(u32::MAX as usize + 1, usize::MAX),
            Err(FrameError::TooLarge { .. })
        ));
    }
    #[cfg(target_pointer_width = "32")]
    {
        assert!(validate_payload_len(usize::MAX - 4, usize::MAX).is_ok());
        assert!(matches!(
            validate_payload_len(usize::MAX - 3, usize::MAX),
            Err(FrameError::TooLarge { .. })
        ));
    }
}
