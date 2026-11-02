use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub enum SecretOutput {
    Stdout,
    File(std::fs::File, bool),
}

pub fn prepare(path: Option<&Path>, terminal: bool) -> anyhow::Result<SecretOutput> {
    if let Some(path) = path {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(path)
            .map_err(|_| anyhow::anyhow!("cannot exclusively create secret output file"))?;
        Ok(SecretOutput::File(file, cfg!(windows)))
    } else if terminal {
        Ok(SecretOutput::Stdout)
    } else {
        anyhow::bail!("secret output unavailable; specify --out-file")
    }
}

pub fn deliver(
    output: SecretOutput,
    secret: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> io::Result<()> {
    match output {
        SecretOutput::Stdout => {
            stdout.write_all(secret.as_bytes())?;
            stdout.write_all(b"\n")
        }
        SecretOutput::File(mut file, warn_windows) => {
            if warn_windows {
                stderr.write_all(b"warning: Windows file ACLs were not tightened\n")?;
            }
            file.write_all(secret.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()
        }
    }
}

pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}
