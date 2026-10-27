//! Parser for the Linux `/sys` **cpulist** format.
//!
//! `/sys/devices/system/node/nodeN/cpulist` (and the sibling files under
//! `/sys/devices/system/cpu/`) encode a set of CPUs as a comma-separated list
//! of single indices and inclusive ranges, e.g. `0-3`, `0,2,4,6`, or
//! `0-1,8-9`. This is the canonical Linux NUMA discovery surface (Drepper
//! §5.3; `Documentation/ABI/stable/sysfs-devices-node`).
//!
//! The parser is pure (no I/O, no syscalls) so it is fully unit-testable on
//! every platform — the file reads that feed it live in the forthcoming
//! `LinuxTopology` (Фаза 1b). It is exposed publicly because it is also useful
//! standalone for tooling that inspects `/proc` / `/sys` cpu masks.

use crate::core::node::CpuId;

const MAX_EXPANDED_CPUS: usize = 1_000_000;

/// Parse a Linux cpulist string into a sorted, de-duplicated list of CPUs.
///
/// Accepts comma-separated tokens, each either a single decimal index (`5`) or
/// an inclusive range (`2-7`). Whitespace around tokens and endpoints is
/// tolerated. Malformed tokens and reversed ranges (`7-2`) are skipped rather
/// than erroring — `/sys` never emits them, and a best-effort discovery should
/// not abort on a single bad line.
///
/// Expansion is capped at 1,000,000 entries counted before de-duplication
/// (repeated tokens consume the budget); a token that would exceed the
/// remaining budget, or a range whose width overflows, is skipped whole.
///
/// ```
/// use shamir_numa::{parse_cpulist, CpuId};
/// assert_eq!(parse_cpulist("0-1,4"), vec![CpuId(0), CpuId(1), CpuId(4)]);
/// assert_eq!(parse_cpulist(""), vec![]);
/// ```
pub fn parse_cpulist(s: &str) -> Vec<CpuId> {
    let mut cpus: Vec<usize> = Vec::new();
    for token in s.trim().split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match token.split_once('-') {
            Some((lo, hi)) => {
                if let (Ok(lo), Ok(hi)) = (lo.trim().parse::<usize>(), hi.trim().parse::<usize>()) {
                    if lo <= hi {
                        let Some(span) = hi.checked_sub(lo).and_then(|width| width.checked_add(1))
                        else {
                            continue;
                        };
                        if span > MAX_EXPANDED_CPUS.saturating_sub(cpus.len()) {
                            continue;
                        }
                        cpus.extend(lo..=hi);
                    }
                }
            }
            None => {
                if let Ok(n) = token.parse::<usize>() {
                    if cpus.len() < MAX_EXPANDED_CPUS {
                        cpus.push(n);
                    }
                }
            }
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    cpus.into_iter().map(CpuId).collect()
}
