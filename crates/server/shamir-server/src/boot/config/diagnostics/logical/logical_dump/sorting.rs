use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use shamir_sync::replication_seed::{decode_canonical, encode_canonical, PortableItem};
use tokio_util::sync::CancellationToken;

type Entry = (Vec<u8>, Vec<u8>);
const ROW_LIMIT: usize = 1024 * 1024;
const RUN_LIMIT: usize = 4 * 1024 * 1024;

/// Bounded binary-carry merge sort. At most 40 live runs (1 TiB ceiling),
/// two merge heads, and a 4 MiB input run; never a whole-table collection.
pub(super) struct Sorter {
    root: PathBuf,
    runs: Vec<Option<PathBuf>>,
    pending: Vec<Entry>,
    bytes: usize,
    total: u64,
    serial: u64,
}

impl Sorter {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            runs: Vec::new(),
            pending: Vec::new(),
            bytes: 0,
            total: 0,
            serial: 0,
        }
    }

    pub(super) async fn push(
        &mut self,
        key: Vec<u8>,
        item: &PortableItem,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let bytes = encode_canonical(item)?;
        if key.len() + bytes.len() > ROW_LIMIT {
            bail!("logical-export scoped row exceeds 1 MiB");
        }
        self.total += (key.len() + bytes.len() + 8) as u64;
        if self.total > 1024 * 1024 * 1024 * 1024 {
            bail!("logical-export sort exceeds 1 TiB");
        }
        self.bytes += key.len() + bytes.len() + std::mem::size_of::<Entry>();
        self.pending.push((key, bytes));
        if self.bytes >= RUN_LIMIT {
            self.flush(cancel).await?;
        }
        Ok(())
    }

    fn name(&mut self) -> PathBuf {
        self.serial += 1;
        self.root.join(format!("run-{}", self.serial))
    }

    async fn flush(&mut self, cancel: &CancellationToken) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.pending.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        if self.pending.windows(2).any(|p| p[0].0 == p[1].0) {
            bail!("duplicate raw record key");
        }
        let mut path = self.name();
        let mut file = File::create(&path)?;
        for entry in self.pending.drain(..) {
            checkpoint(cancel).await?;
            write_entry(&mut file, &entry)?;
        }
        drop(file);
        self.bytes = 0;
        let mut level = 0;
        loop {
            if level >= 40 {
                bail!("logical-export sort run limit exceeded");
            }
            if self.runs.len() <= level {
                self.runs.push(None);
            }
            let Some(previous) = self.runs[level].take() else {
                self.runs[level] = Some(path);
                break;
            };
            let next = self.name();
            merge(&previous, &path, &next, cancel).await?;
            fs::remove_file(previous)?;
            fs::remove_file(path)?;
            path = next;
            level += 1;
        }
        Ok(())
    }

    pub(super) async fn finish(mut self, cancel: &CancellationToken) -> Result<Option<File>> {
        self.flush(cancel).await?;
        let mut final_path: Option<PathBuf> = None;
        for level in 0..self.runs.len() {
            if let Some(path) = self.runs[level].take() {
                final_path = Some(if let Some(previous) = final_path {
                    let next = self.name();
                    merge(&previous, &path, &next, cancel).await?;
                    fs::remove_file(previous)?;
                    fs::remove_file(path)?;
                    next
                } else {
                    path
                });
            }
        }
        final_path.map(File::open).transpose().map_err(Into::into)
    }
}

pub(super) async fn checkpoint(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("logical-export cancelled");
    }
    tokio::task::yield_now().await;
    if cancel.is_cancelled() {
        bail!("logical-export cancelled");
    }
    Ok(())
}

fn write_entry(file: &mut File, entry: &Entry) -> Result<()> {
    file.write_all(&u32::try_from(entry.0.len())?.to_be_bytes())?;
    file.write_all(&u32::try_from(entry.1.len())?.to_be_bytes())?;
    file.write_all(&entry.0)?;
    file.write_all(&entry.1)?;
    Ok(())
}

fn read_entry(file: &mut File) -> Result<Option<Entry>> {
    let mut lengths = [0; 8];
    if file.read(&mut lengths[..1])? == 0 {
        return Ok(None);
    }
    file.read_exact(&mut lengths[1..])?;
    let key_len = u32::from_be_bytes(lengths[..4].try_into()?) as usize;
    let value_len = u32::from_be_bytes(lengths[4..].try_into()?) as usize;
    if key_len.saturating_add(value_len) > ROW_LIMIT {
        bail!("sort entry exceeds scoped row limit");
    }
    let mut key = vec![0; key_len];
    let mut value = vec![0; value_len];
    file.read_exact(&mut key)?;
    file.read_exact(&mut value)?;
    Ok(Some((key, value)))
}

pub(super) fn next_item(file: &mut File) -> Result<Option<PortableItem>> {
    read_entry(file)?
        .map(|(_, bytes)| decode_canonical(&bytes).map_err(Into::into))
        .transpose()
}

async fn merge(left: &Path, right: &Path, output: &Path, cancel: &CancellationToken) -> Result<()> {
    let mut left = File::open(left)?;
    let mut right = File::open(right)?;
    let mut output = File::create(output)?;
    let mut a = read_entry(&mut left)?;
    let mut b = read_entry(&mut right)?;
    while a.is_some() || b.is_some() {
        checkpoint(cancel).await?;
        if let (Some(a), Some(b)) = (&a, &b) {
            if a.0 == b.0 {
                bail!("duplicate raw record key");
            }
        }
        let take_left = match (&a, &b) {
            (Some(a), Some(b)) => a.0 < b.0,
            (Some(_), None) => true,
            _ => false,
        };
        if take_left {
            write_entry(&mut output, a.as_ref().expect("left head present"))?;
            a = read_entry(&mut left)?;
        } else {
            write_entry(&mut output, b.as_ref().expect("right head present"))?;
            b = read_entry(&mut right)?;
        }
    }
    Ok(())
}
