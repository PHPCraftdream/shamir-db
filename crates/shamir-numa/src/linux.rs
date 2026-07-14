//! `LinuxTopology` — production [`Topology`] impl backed by
//! `/sys/devices/system/node/` and `libc::sched_*`.
//!
//! Enabled only under `cfg(target_os = "linux")`.
//!
//! # Discovery surface
//!
//! The canonical Linux NUMA discovery surface is the `sysfs` hierarchy at
//! `/sys/devices/system/node/` (Drepper, *What Every Programmer Should Know
//! About Memory*, §5.3; `Documentation/ABI/stable/sysfs-devices-node`).
//! We read two files:
//!
//! * `/sys/devices/system/node/online` — a cpulist of node indices present
//!   and online (e.g. `0-1` on a dual-socket host, `0` on single-socket).
//! * `/sys/devices/system/node/nodeN/cpulist` — the logical CPUs belonging
//!   to node `N` (e.g. `0-11,24-35`).
//!
//! Both files use the same cpulist encoding already parsed by
//! [`parse_cpulist`](crate::parse_cpulist).

use std::io;
use std::mem;

use shamir_collections::TFxMap;

use crate::cpulist::parse_cpulist;
use crate::error::AffinityError;
use crate::node::{CpuId, NodeId};
use crate::topology::Topology;

/// Production topology built from `/sys/devices/system/node/` on Linux.
///
/// Probe via [`LinuxTopology::probe`]. The result is `Send + Sync` and cheap
/// to query — all data is copied out of `/sys` at construction time, so
/// read hot-paths never touch the kernel again.
pub struct LinuxTopology {
    /// `cores[i]` is the list of logical CPUs on NUMA node `i`.
    cores: Vec<Vec<CpuId>>,
    /// Reverse index: CPU → node, for fast [`current_node`](Topology::current_node) lookup.
    cpu_to_node: TFxMap<CpuId, NodeId>,
}

impl LinuxTopology {
    /// Probe `/sys/devices/system/node/` and build the topology.
    ///
    /// Returns [`AffinityError::Unsupported`] when the directory or the
    /// `online` file are missing — most commonly inside containers that do
    /// not mount sysfs, or on kernels compiled without NUMA support.
    ///
    /// A missing per-node `cpulist` is treated as an empty CPU list for that
    /// node (best-effort) rather than a hard error.
    pub fn probe() -> Result<Self, AffinityError> {
        // 1. Read the list of online NUMA nodes.
        let online_path = "/sys/devices/system/node/online";
        let online_raw = fs_read_trim(online_path)?;
        // parse_cpulist reuses the same format the node-online file uses.
        let node_ids: Vec<usize> = parse_cpulist(&online_raw)
            .into_iter()
            .map(|c| c.0)
            .collect();

        if node_ids.is_empty() {
            return Err(AffinityError::Unsupported);
        }

        // 2. For each node read its cpulist.
        //    `cores` is indexed by a dense 0-based slot (not the raw node id),
        //    so we sort node_ids first and assign slots in order.
        let mut node_ids_sorted = node_ids;
        node_ids_sorted.sort_unstable();

        let mut cores: Vec<Vec<CpuId>> = Vec::with_capacity(node_ids_sorted.len());
        for &nid in &node_ids_sorted {
            let cpulist_path = format!("/sys/devices/system/node/node{nid}/cpulist");
            let cpus = match fs_read_trim(&cpulist_path) {
                Ok(raw) => parse_cpulist(&raw),
                // Missing file — container or NUMA-less kernel; keep node but empty.
                Err(_) => Vec::new(),
            };
            cores.push(cpus);
        }

        // 3. Build the reverse CPU→node map.
        let mut cpu_to_node: TFxMap<CpuId, NodeId> = TFxMap::default();
        for (slot, cpus) in cores.iter().enumerate() {
            let node = NodeId(slot);
            for &cpu in cpus {
                cpu_to_node.insert(cpu, node);
            }
        }

        Ok(Self { cores, cpu_to_node })
    }
}

impl Topology for LinuxTopology {
    fn num_nodes(&self) -> usize {
        self.cores.len()
    }

    fn cores_on_node(&self, node: NodeId) -> &[CpuId] {
        self.cores.get(node.0).map(Vec::as_slice).unwrap_or(&[])
    }

    fn current_node(&self) -> NodeId {
        // SAFETY: sched_getcpu() is a pure read-only vDSO call on Linux; it
        // takes no pointer arguments and cannot fault. The return value is a
        // non-negative CPU index on success, -1 on error (we fall back to 0).
        let cpu_raw = unsafe { libc::sched_getcpu() };
        if cpu_raw < 0 {
            return NodeId(0);
        }
        let cpu = CpuId(cpu_raw as usize);
        // Miss → thread is on an unknown CPU (container CPU-set changed after
        // probe). Degrade to node 0 rather than panicking.
        self.cpu_to_node.get(&cpu).copied().unwrap_or(NodeId(0))
    }

    fn pin_current_thread_to_node(&self, node: NodeId) -> Result<(), AffinityError> {
        let cpus = self
            .cores
            .get(node.0)
            .ok_or(AffinityError::NodeOutOfRange {
                requested: node.0,
                available: self.cores.len(),
            })?;

        // Build a zero-initialised cpu_set_t and add every CPU of this node.
        // SAFETY:
        //   - `cpu_set` is stack-allocated and fully initialised to zero via
        //     `libc::CPU_ZERO` before any CPU bits are set — no uninitialised
        //     bytes are read.
        //   - The pointer passed to `sched_setaffinity` is valid for the entire
        //     duration of the syscall (it lives on the current stack frame).
        //   - `mem::size_of::<libc::cpu_set_t>()` matches the C ABI size that
        //     the kernel expects for the `cpusetsize` argument (glibc hard-codes
        //     this as `sizeof(cpu_set_t)` = 128 bytes on 64-bit Linux).
        //   - `pid = 0` targets the calling thread, which is always valid.
        let ret = unsafe {
            let mut cpu_set: libc::cpu_set_t = mem::zeroed();
            libc::CPU_ZERO(&mut cpu_set);
            for &cpu in cpus {
                libc::CPU_SET(cpu.0, &mut cpu_set);
            }
            libc::sched_setaffinity(
                0, // 0 = current thread
                mem::size_of::<libc::cpu_set_t>(),
                &cpu_set as *const libc::cpu_set_t,
            )
        };

        if ret != 0 {
            Err(AffinityError::Syscall(io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read a sysfs file, trim trailing whitespace/newlines, and return its content.
/// Returns [`AffinityError::Unsupported`] when the file does not exist, and
/// [`AffinityError::Syscall`] for other I/O errors.
fn fs_read_trim(path: &str) -> Result<String, AffinityError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s.trim_end().to_owned()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(AffinityError::Unsupported),
        Err(e) => Err(AffinityError::Syscall(e)),
    }
}

// ---------------------------------------------------------------------------
// Tests (compiled only on Linux)
// ---------------------------------------------------------------------------

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn probe_on_real_linux_host_succeeds() {
        let topo = LinuxTopology::probe().expect("Linux host must expose /sys/devices/system/node");
        assert!(topo.num_nodes() >= 1);
        let node0 = NodeId(0);
        assert!(
            !topo.cores_on_node(node0).is_empty(),
            "node 0 must have at least one CPU"
        );
    }

    #[test]
    fn current_node_is_in_range() {
        let topo = LinuxTopology::probe().unwrap();
        let n = topo.current_node();
        assert!(n.0 < topo.num_nodes());
    }
}
