use std::collections::BTreeSet;
use thiserror::Error;
use wasm_encoder::Function;

use crate::back::runtime::{
    emit_append_byte, emit_check_range, emit_compare_bytes, emit_copy_cells, emit_decode_root,
    emit_encode_root, emit_read_be, emit_read_byte, emit_store_cell, emit_validate_utf8,
    DecodeEmit, EncodeEmit, RuntimeCaps, RuntimeError,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct HelperId(pub u32);

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum HelperOrderError {
    #[error("helper dependency references an unknown node")]
    UnknownDependency,
    #[error("helper dependency graph contains a cycle")]
    Cycle,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HelperGraph {
    dependencies: Vec<Vec<HelperId>>,
}

impl HelperGraph {
    pub fn add(&mut self, dependencies: &[HelperId]) -> HelperId {
        let id = HelperId(u32::try_from(self.dependencies.len()).expect("helper count fits u32"));
        self.dependencies.push(dependencies.to_vec());
        id
    }

    pub fn dependencies(&self, id: HelperId) -> Option<&[HelperId]> {
        self.dependencies.get(id.0 as usize).map(Vec::as_slice)
    }

    #[cfg(test)]
    pub fn add_dependency(&mut self, id: HelperId, dependency: HelperId) {
        self.dependencies[id.0 as usize].push(dependency);
    }

    pub fn order(&self, roots: &[HelperId]) -> Result<Vec<HelperId>, HelperOrderError> {
        let mut selected = BTreeSet::new();
        let mut pending = roots.to_vec();
        while let Some(id) = pending.pop() {
            let Some(dependencies) = self.dependencies(id) else {
                return Err(HelperOrderError::UnknownDependency);
            };
            if selected.insert(id) {
                pending.extend(dependencies.iter().copied());
            }
        }
        let mut indegree = vec![0usize; self.dependencies.len()];
        let mut dependents = vec![Vec::new(); self.dependencies.len()];
        for id in selected.iter().copied() {
            for dependency in self
                .dependencies(id)
                .ok_or(HelperOrderError::UnknownDependency)?
            {
                if selected.contains(dependency) {
                    indegree[id.0 as usize] += 1;
                    dependents[dependency.0 as usize].push(id);
                }
            }
        }
        let mut ready: BTreeSet<HelperId> = selected
            .iter()
            .copied()
            .filter(|id| indegree[id.0 as usize] == 0)
            .collect();
        let mut result = Vec::with_capacity(selected.len());
        while let Some(id) = ready.pop_first() {
            result.push(id);
            for dependent in &dependents[id.0 as usize] {
                indegree[dependent.0 as usize] -= 1;
                if indegree[dependent.0 as usize] == 0 {
                    ready.insert(*dependent);
                }
            }
        }
        if result.len() != selected.len() {
            return Err(HelperOrderError::Cycle);
        }
        Ok(result)
    }
}

pub(crate) fn emit_helper_order(
    graph: &HelperGraph,
    roots: &[HelperId],
) -> Result<Vec<HelperId>, HelperOrderError> {
    graph.order(roots)
}

pub(super) type OrderedHelpers = (Vec<(u32, Function)>, u32, u32, u32, u32);

/// Registers each codec primitive plus the iterative decoder as a
/// `HelperGraph` node with its real dependencies, orders them deterministically
/// (BTree-backed), and returns the emitted bodies with their type indices in
/// dependency-first order together with the decoder/encoder module indices.
pub(super) fn ordered_helper_functions(
    scratch: u32,
    sort_base: u32,
    caps: RuntimeCaps,
    records: bool,
    import_count: u32,
) -> Result<OrderedHelpers, RuntimeError> {
    const TYPE_RANGE: u32 = 0;
    const TYPE_SMALL: u32 = 1;
    const TYPE_READ_BE: u32 = 3;
    const TYPE_STORE_CELL: u32 = 5;
    const TYPE_APPEND: u32 = 6;
    const TYPE_DECODE: u32 = 9;
    const TYPE_ENCODE: u32 = 9;

    let mut graph = HelperGraph::default();
    let range = graph.add(&[]);
    let utf8 = graph.add(&[]);
    let compare = graph.add(&[]);
    let read_byte = graph.add(&[]);
    let read_be = graph.add(&[read_byte]);
    let copy = graph.add(&[]);
    let store = graph.add(&[]);
    let append = graph.add(&[]);
    // the decoder calls range validation, byte/be reads, UTF-8 validation,
    // byte comparison (duplicate keys) and cell stores; it allocates heap
    // copies through the allocator and raises through the error helper, both
    // emitted after the graph order is fixed. The encoder appends to the
    // output buffer and compares map-key bytes for the insertion sort.
    let decode = graph.add(&[range, read_byte, read_be, utf8, compare, store]);
    let encode = graph.add(&[append, compare]);
    let roots = [
        range, utf8, compare, read_byte, read_be, copy, store, append, decode, encode,
    ];
    let order = emit_helper_order(&graph, &roots)?;

    let helper_count = order.len() as u32;
    // defined functions start after the host imports (`raise_error` plus the
    // stage-2 database imports of a procedure module)
    let error_raiser_index = import_count + helper_count;
    let alloc_index = error_raiser_index + 1;
    let index_of = |helper: HelperId| -> Result<u32, RuntimeError> {
        let position = order
            .iter()
            .position(|&candidate| candidate == helper)
            .ok_or(RuntimeError::UnknownHelper)?;
        u32::try_from(position)
            .map(|position| import_count + position)
            .map_err(|_| RuntimeError::UnknownHelper)
    };
    let decode_params = DecodeEmit {
        scratch,
        raise_index: error_raiser_index,
        alloc_index,
        check_range_index: index_of(range)?,
        read_byte_index: index_of(read_byte)?,
        read_be_index: index_of(read_be)?,
        compare_index: index_of(compare)?,
        utf8_index: index_of(utf8)?,
        store_cell_index: index_of(store)?,
        caps,
    };
    let encode_params = EncodeEmit {
        scratch,
        sort_base,
        raise_index: error_raiser_index,
        append_index: index_of(append)?,
        compare_index: index_of(compare)?,
        caps,
        records,
    };

    let mut functions: Vec<(u32, Function)> = Vec::with_capacity(order.len());
    for helper in &order {
        let entry = match *helper {
            id if id == range => (TYPE_RANGE, emit_check_range()),
            id if id == utf8 => (TYPE_SMALL, emit_validate_utf8()),
            id if id == compare => (TYPE_COMPARE, emit_compare_bytes()),
            id if id == read_byte => (TYPE_SMALL, emit_read_byte()),
            id if id == read_be => (TYPE_READ_BE, emit_read_be()),
            id if id == copy => (TYPE_VOID_THREE_I32, emit_copy_cells()),
            id if id == store => (TYPE_STORE_CELL, emit_store_cell()),
            id if id == append => (TYPE_APPEND, emit_append_byte()),
            id if id == decode => (TYPE_DECODE, emit_decode_root(&decode_params)),
            id if id == encode => (TYPE_ENCODE, emit_encode_root(&encode_params)),
            _ => return Err(RuntimeError::UnknownHelper),
        };
        functions.push(entry);
    }
    let decode_index = index_of(decode)?;
    let encode_index = index_of(encode)?;
    let compare_index = index_of(compare)?;
    let copy_cells_index = index_of(copy)?;
    Ok((
        functions,
        decode_index,
        encode_index,
        compare_index,
        copy_cells_index,
    ))
}

const TYPE_VOID_THREE_I32: u32 = 4;
/// Runtime byte-compare helper type (`[i32,i32] -> [i32]`, position 2).
const TYPE_COMPARE: u32 = 2;
