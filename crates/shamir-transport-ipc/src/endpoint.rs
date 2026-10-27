use std::io;

/// Validate a raw platform IPC address before any operating-system operation.
pub fn validate_endpoint(addr: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::path::{Component, Path};

        let path = Path::new(addr);
        let max = if cfg!(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )) {
            104
        } else {
            108
        };
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix IPC address must be an absolute path",
            ));
        }
        if path.components().any(|part| part == Component::ParentDir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix IPC address must not contain a parent-directory component",
            ));
        }
        if addr.as_bytes().contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix IPC address must not contain NUL",
            ));
        }
        if addr.len() >= max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Unix IPC address must be shorter than SUN_PATH_LEN ({max} bytes)"),
            ));
        }
    }
    #[cfg(windows)]
    {
        const PREFIX: &str = r"\\.\pipe\";
        let (prefix, rest) = if addr.len() >= PREFIX.len() && addr.is_char_boundary(PREFIX.len()) {
            addr.split_at(PREFIX.len())
        } else {
            (addr, "")
        };
        if !prefix.eq_ignore_ascii_case(PREFIX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows IPC address must start with the required \\\\.\\pipe\\ prefix",
            ));
        }
        if rest.is_empty() || rest.contains('\\') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows IPC pipe name must have a non-empty single-component name",
            ));
        }
        if addr.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows IPC address must not contain NUL",
            ));
        }
        if addr.encode_utf16().count() > 256 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows IPC address must not exceed 256 UTF-16 code units",
            ));
        }
    }
    Ok(())
}
