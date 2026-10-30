use super::wipe;

#[test]
fn wipe_clears_live_string_without_releasing_capacity() {
    for text in ["", "пароль 🔐"] {
        let mut buffer = String::with_capacity(64);
        buffer.push_str(text);
        let capacity = buffer.capacity();
        wipe(&mut buffer);
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), capacity);
        // Safe String APIs cannot inspect the now-uninitialized spare capacity.
    }
}
