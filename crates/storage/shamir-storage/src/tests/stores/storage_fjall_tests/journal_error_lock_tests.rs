//! Guards RV-STORAGE-EC3A171B: fjall before 3.1.7 discards the journal write
//! result in `WriteBatch::commit`. The remedy is the locked fjall version, so
//! a lockfile downgrade must fail loudly. A second guard pins 3.1.13, which
//! fixes the `Drop` hang of the database (a full bounded close channel made
//! `DatabaseInner::drop` spin and keep the directory lock).

use std::fs;

const MIN_FJALL: (u64, u64, u64) = (3, 1, 7);
const MIN_FJALL_DROP_FIX: (u64, u64, u64) = (3, 1, 13);

fn locked_fjall_version() -> (u64, u64, u64) {
    let lock = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../Cargo.lock"))
        .expect("read the workspace Cargo.lock");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "name = \"fjall\"" {
            let version = lines
                .next()
                .and_then(|l| l.trim().strip_prefix("version = \""))
                .and_then(|l| l.strip_suffix('"'))
                .expect("fjall version line");
            let mut parts = version
                .split('.')
                .map(|p| p.parse::<u64>().expect("numeric"));
            return (
                parts.next().expect("major"),
                parts.next().expect("minor"),
                parts.next().expect("patch"),
            );
        }
    }
    panic!("fjall is not in Cargo.lock");
}

#[test]
fn rv_storage_ec3a171b_locked_fjall_propagates_journal_write_errors() {
    assert!(
        locked_fjall_version() >= MIN_FJALL,
        "fjall below 3.1.7 discards the journal write result in WriteBatch::commit"
    );
}

#[test]
fn locked_fjall_has_the_database_drop_fix() {
    assert!(
        locked_fjall_version() >= MIN_FJALL_DROP_FIX,
        "fjall below 3.1.13 can hang in DatabaseInner::drop and keep the directory lock"
    );
}
