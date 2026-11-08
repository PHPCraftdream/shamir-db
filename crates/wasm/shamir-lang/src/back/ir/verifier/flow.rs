use crate::back::ir::module::IrBlock;
use crate::back::ir::module::IrFunction;
use crate::back::ir::ops::terminator_targets;
use crate::back::ir::ops::BlockKind;
use crate::back::ir::ops::InstKind;

use super::kinds::slot_limit;
use super::out_of_range;
use super::tile_error;
use super::VerifierError;

/// Safety ceiling for the iterative fixpoint walks.
const MAX_DOMINATOR_WORK: u64 = 4_000_000;

// Rule 7: block 0 starts at 0, each block continues where the previous ends,
// and the last block ends exactly at the arena length.
pub(crate) fn check_tiling(
    index: u32,
    blocks: &[IrBlock],
    inst_len: usize,
) -> Result<(), VerifierError> {
    let mut expected = 0u32;
    for (block_index, block) in blocks.iter().enumerate() {
        if block.start.0 != expected {
            return Err(tile_error(
                index,
                block_index,
                format!(
                    "block starts at {} but tiling expects {}",
                    block.start.0, expected
                ),
            ));
        }
        let end = expected.checked_add(block.inst_count).ok_or_else(|| {
            tile_error(
                index,
                block_index,
                String::from("block range overflows u32"),
            )
        })?;
        expected = end;
    }
    if expected as u64 != inst_len as u64 {
        let last = blocks.len().saturating_sub(1);
        return Err(tile_error(
            index,
            last,
            format!(
                "tiling ends at {} but arena holds {} insts",
                expected, inst_len
            ),
        ));
    }
    Ok(())
}

// Rule 4: branches go forward or back to a loop header only.
pub(crate) fn check_branch_directions(
    function_index: u32,
    function: &IrFunction,
) -> Result<(), VerifierError> {
    for block_index in 0..function.blocks.len() {
        let block = function.blocks.get(block_index).ok_or_else(|| {
            tile_error(function_index, block_index, String::from("missing block"))
        })?;
        let mut targets = Vec::new();
        terminator_targets(&block.terminator, &mut targets);
        check_targets(function_index, function, block_index, &targets)?;
        for target in targets {
            let target_block = function.blocks.get(target as usize).ok_or_else(|| {
                out_of_range(
                    function_index,
                    block_index,
                    0,
                    format!("block target {}", target),
                )
            })?;
            let forward = target > block_index as u32;
            let loop_header = target_block.kind == BlockKind::Loop;
            if !forward && !loop_header {
                return Err(VerifierError::BadBranchDirection {
                    function: function_index,
                    block: block_index as u32,
                    target,
                });
            }
        }
    }
    Ok(())
}

// Rule 1 for branch targets.
fn check_targets(
    function_index: u32,
    function: &IrFunction,
    block_index: usize,
    targets: &[u32],
) -> Result<(), VerifierError> {
    for target in targets {
        if *target as u64 >= function.blocks.len() as u64 {
            return Err(out_of_range(
                function_index,
                block_index,
                0,
                format!("block target {}", target),
            ));
        }
    }
    Ok(())
}

// Rule 5: each region is a contiguous block range led by its Loop block.
pub(crate) fn check_regions(
    function_index: u32,
    function: &IrFunction,
) -> Result<(), VerifierError> {
    let mut regions: Vec<(u32, usize, usize)> = Vec::new(); // (region, first, last)
    for (block_index, block) in function.blocks.iter().enumerate() {
        if block.kind == BlockKind::Loop && block.region.is_none() {
            return Err(VerifierError::RegionNotContiguous {
                function: function_index,
                region: u32::MAX,
                detail: String::from("loop block without a region id"),
            });
        }
        if let Some(region) = block.region {
            match regions
                .iter_mut()
                .find(|(existing, _, _)| *existing == region)
            {
                Some((_, _, last)) => *last = block_index,
                None => regions.push((region, block_index, block_index)),
            }
        }
    }
    for (region, first, last) in regions {
        let count = last - first + 1;
        let matching = function
            .blocks
            .iter()
            .skip(first)
            .take(count)
            .filter(|block| block.region == Some(region))
            .count();
        if matching != count
            || function
                .blocks
                .iter()
                .filter(|block| block.region == Some(region))
                .count()
                != count
        {
            return Err(VerifierError::RegionNotContiguous {
                function: function_index,
                region,
                detail: format!("blocks {}..={} are not exactly the region", first, last),
            });
        }
        let leader = function.blocks.get(first).ok_or_else(|| {
            out_of_range(
                function_index,
                first,
                0,
                format!("region {} leader", region),
            )
        })?;
        if leader.kind != BlockKind::Loop {
            return Err(VerifierError::RegionNotContiguous {
                function: function_index,
                region,
                detail: format!("first block {} of the region is not a loop header", first),
            });
        }
    }
    Ok(())
}

// Rule 6: every block is reachable from block 0 over terminator edges.
pub(crate) fn check_reachable(
    function_index: u32,
    function: &IrFunction,
) -> Result<(), VerifierError> {
    if function.blocks.is_empty() {
        return Err(VerifierError::BadTerminator {
            function: function_index,
            block: 0,
            detail: String::from("function without blocks"),
        });
    }
    let mut reachable = vec![false; function.blocks.len()];
    let mut stack = vec![0usize];
    while let Some(block_index) = stack.pop() {
        if reachable.get(block_index).copied().unwrap_or(true) {
            continue;
        }
        if let Some(seen) = reachable.get_mut(block_index) {
            *seen = true;
        }
        let block = match function.blocks.get(block_index) {
            Some(block) => block,
            None => {
                return Err(out_of_range(
                    function_index,
                    block_index,
                    0,
                    String::from("block index"),
                ))
            }
        };
        let mut targets = Vec::new();
        terminator_targets(&block.terminator, &mut targets);
        for target in targets {
            if target as u64 >= function.blocks.len() as u64 {
                return Err(out_of_range(
                    function_index,
                    block_index,
                    0,
                    format!("block target {}", target),
                ));
            }
            stack.push(target as usize);
        }
    }
    for (block_index, seen) in reachable.iter().enumerate() {
        if !seen {
            return Err(VerifierError::UnreachableBlock {
                function: function_index,
                block: block_index as u32,
            });
        }
    }
    Ok(())
}

// Rule 8: definite-initialization dataflow. For every slot, `init_in[b]` is
// the intersection of its predecessors' `init_out` sets (params are always
// defined), `init_out[b]` adds the LocalSets of `b`; a LocalGet is valid iff
// the slot is a parameter or already in the running init set at that point.
pub(crate) fn check_local_domination(
    function_index: u32,
    function: &IrFunction,
) -> Result<(), VerifierError> {
    let block_count = function.blocks.len();
    let slot_count = slot_limit(function)?;
    let params = u32::try_from(function.params.len()).map_err(|_| VerifierError::Internal {
        function: function_index,
        detail: String::from("parameter count overflow"),
    })?;
    // Predecessors per block (branch targets are range-checked here too).
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); block_count];
    for block_index in 0..block_count {
        let block = function.blocks.get(block_index).ok_or_else(|| {
            tile_error(function_index, block_index, String::from("missing block"))
        })?;
        let mut targets = Vec::new();
        terminator_targets(&block.terminator, &mut targets);
        for target in targets {
            if target as u64 >= block_count as u64 {
                return Err(out_of_range(
                    function_index,
                    block_index,
                    0,
                    format!("block target {}", target),
                ));
            }
            if let Some(entry) = predecessors.get_mut(target as usize) {
                entry.push(block_index);
            }
        }
    }
    // Per-block LocalSet slot lists.
    let mut set_slots: Vec<Vec<u32>> = vec![Vec::new(); block_count];
    for block_index in 0..block_count {
        let block = function.blocks.get(block_index).ok_or_else(|| {
            tile_error(function_index, block_index, String::from("missing block"))
        })?;
        for offset in 0..block.inst_count {
            let inst_id = block.start.0.checked_add(offset).ok_or_else(|| {
                tile_error(function_index, block_index, String::from("range overflow"))
            })?;
            let inst = function.insts.get(inst_id as usize).ok_or_else(|| {
                out_of_range(
                    function_index,
                    block_index,
                    inst_id,
                    String::from("instruction id"),
                )
            })?;
            if let InstKind::LocalSet(slot) = inst.kind {
                match set_slots.get_mut(block_index) {
                    Some(entries) => entries.push(slot),
                    None => {
                        return Err(tile_error(
                            function_index,
                            block_index,
                            String::from("missing block"),
                        ))
                    }
                }
            }
        }
    }
    // Fixpoint over reverse postorder: init_in[b] = AND over preds' out.
    let order = reverse_postorder(function, block_count)?;
    let mut init_in: Vec<Vec<bool>> = vec![vec![false; slot_count as usize]; block_count];
    let mut init_out: Vec<Vec<bool>> = vec![vec![false; slot_count as usize]; block_count];
    let mut work: u64 = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for block_index in &order {
            let block_index = *block_index;
            let preds = predecessors
                .get(block_index)
                .ok_or_else(|| VerifierError::Internal {
                    function: function_index,
                    detail: String::from("predecessor table missing a block"),
                })?;
            let mut next = vec![true; slot_count as usize];
            for pred in preds {
                work += slot_count as u64;
                let pred_out = init_out.get(*pred).ok_or_else(|| VerifierError::Internal {
                    function: function_index,
                    detail: String::from("init out-set missing"),
                })?;
                for (cell, pred_bit) in next.iter_mut().zip(pred_out.iter()) {
                    *cell = *cell && *pred_bit;
                }
            }
            // A block with no predecessors (other than the entry) starts
            // empty; unreachable blocks are rejected elsewhere. `next` is
            // the in-set; snapshot it, then add this block's LocalSets for
            // the out-set.
            let mut out = next.clone();
            for slot in set_slots
                .get(block_index)
                .ok_or_else(|| VerifierError::Internal {
                    function: function_index,
                    detail: String::from("set-slot table missing a block"),
                })?
                .iter()
            {
                if let Some(cell) = out.get_mut(*slot as usize) {
                    *cell = true;
                }
            }
            work += slot_count as u64;
            let stored_out =
                init_out
                    .get_mut(block_index)
                    .ok_or_else(|| VerifierError::Internal {
                        function: function_index,
                        detail: String::from("init out-set missing"),
                    })?;
            if *stored_out != out {
                *stored_out = out;
                changed = true;
            }
            if let Some(row) = init_in.get_mut(block_index) {
                *row = next;
            }
            if work > MAX_DOMINATOR_WORK {
                return Err(VerifierError::Internal {
                    function: function_index,
                    detail: String::from("init fixpoint exceeded the work ceiling"),
                });
            }
        }
    }
    // Walk each block with the running init set: reads need an earlier
    // in-block set or membership in the block's in-set.
    for block_index in 0..block_count {
        let block = function.blocks.get(block_index).ok_or_else(|| {
            tile_error(function_index, block_index, String::from("missing block"))
        })?;
        let mut running =
            init_in
                .get(block_index)
                .cloned()
                .ok_or_else(|| VerifierError::Internal {
                    function: function_index,
                    detail: String::from("init in-set missing"),
                })?;
        for offset in 0..block.inst_count {
            let inst_id = block.start.0.checked_add(offset).ok_or_else(|| {
                tile_error(function_index, block_index, String::from("range overflow"))
            })?;
            let inst = function.insts.get(inst_id as usize).ok_or_else(|| {
                out_of_range(
                    function_index,
                    block_index,
                    inst_id,
                    String::from("instruction id"),
                )
            })?;
            match &inst.kind {
                InstKind::LocalSet(slot) => {
                    if let Some(cell) = running.get_mut(*slot as usize) {
                        *cell = true;
                    }
                }
                InstKind::LocalGet(slot) => {
                    if *slot < params {
                        continue;
                    }
                    let initialized = running.get(*slot as usize).copied().unwrap_or(false);
                    if !initialized {
                        return Err(VerifierError::LocalNotDominated {
                            function: function_index,
                            block: block_index as u32,
                            inst: inst_id,
                            slot: *slot,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// Iterative postorder DFS from block 0, reversed.
fn reverse_postorder(
    function: &IrFunction,
    block_count: usize,
) -> Result<Vec<usize>, VerifierError> {
    if block_count == 0 {
        return Ok(Vec::new());
    }
    let mut postorder = Vec::with_capacity(block_count);
    let mut visited = vec![false; block_count];
    let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
    visited[0] = true;
    while let Some((block_index, child)) = stack.pop() {
        let block = function
            .blocks
            .get(block_index)
            .ok_or_else(|| tile_error(0, block_index, String::from("missing block")))?;
        let mut targets = Vec::new();
        terminator_targets(&block.terminator, &mut targets);
        let mut pushed = false;
        for target in targets.iter().skip(child) {
            let target = *target as usize;
            if target >= block_count || visited.get(target).copied().unwrap_or(true) {
                continue;
            }
            if let Some(seen) = visited.get_mut(target) {
                *seen = true;
            }
            stack.push((block_index, child + 1));
            stack.push((target, 0));
            pushed = true;
            break;
        }
        if !pushed {
            postorder.push(block_index);
        }
    }
    postorder.reverse();
    Ok(postorder)
}
