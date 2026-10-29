//! Bounded bootstrap credential input. The allocation is pre-sized and
//! zeroized on success and on every error path; this is best-effort memory
//! hygiene and cannot erase copies made by the OS or the input source.

use std::io::BufRead;

use zeroize::Zeroizing;

const MAX_PASSWORD_BYTES: usize = 4096;

/// Read exactly one line, preserving all whitespace except its LF or CRLF.
/// EOF after nonempty bytes is a valid unterminated line.
pub(crate) fn read_password(mut input: impl BufRead) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let mut password = Zeroizing::new(Vec::with_capacity(MAX_PASSWORD_BYTES + 2));
    loop {
        let available = input
            .fill_buf()
            .map_err(|_| anyhow::anyhow!("cannot read bootstrap password from stdin"))?;
        if available.is_empty() {
            break;
        }
        let amount = available
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if amount > MAX_PASSWORD_BYTES + 2 - password.len() {
            anyhow::bail!("bootstrap password from stdin exceeds 4096 bytes");
        }
        let terminated = available[amount - 1] == b'\n';
        password.extend_from_slice(&available[..amount]);
        input.consume(amount);
        if !terminated && password.len() > MAX_PASSWORD_BYTES + 1 {
            anyhow::bail!("bootstrap password from stdin exceeds 4096 bytes");
        }
        if terminated {
            password.pop();
            if password.last() == Some(&b'\r') {
                password.pop();
            }
            break;
        }
    }
    if password.is_empty() {
        anyhow::bail!("bootstrap password from stdin is empty");
    }
    if password.len() > MAX_PASSWORD_BYTES {
        anyhow::bail!("bootstrap password from stdin exceeds 4096 bytes");
    }
    if std::str::from_utf8(&password).is_err() {
        anyhow::bail!("bootstrap password from stdin is not valid UTF-8");
    }
    Ok(password)
}
