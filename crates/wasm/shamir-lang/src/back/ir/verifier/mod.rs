use crate::back::ir::module::IrFunction;
use crate::back::ir::module::IrModule;
use thiserror::Error;

mod flow;
mod kinds;

pub(super) use flow::check_branch_directions;
pub(super) use flow::check_local_domination;
pub(super) use flow::check_reachable;
pub(super) use flow::check_regions;
pub(super) use flow::check_tiling;
pub(super) use kinds::verify_block;

/// Verification failure; every variant names the offending function, block
/// and instruction where applicable. Maps to `CompileError::InternalInvariant`.
#[derive(Debug, Error)]
pub enum VerifierError {
    #[error("function {function} block {block} inst {inst}: out-of-range id: {detail}")]
    OutOfRangeIds {
        function: u32,
        block: u32,
        inst: u32,
        detail: String,
    },
    #[error("function {function} block {block} inst {inst}: operand from another block: {detail}")]
    OperandNotInBlock {
        function: u32,
        block: u32,
        inst: u32,
        detail: String,
    },
    #[error("function {function} block {block} inst {inst}: kind mismatch: {detail}")]
    KindMismatch {
        function: u32,
        block: u32,
        inst: u32,
        detail: String,
    },
    #[error("function {function} block {block}: bad terminator: {detail}")]
    BadTerminator {
        function: u32,
        block: u32,
        detail: String,
    },
    #[error(
        "function {function} block {block}: branch to earlier plain block {target} (back edge only to loop headers)"
    )]
    BadBranchDirection {
        function: u32,
        block: u32,
        target: u32,
    },
    #[error(
        "function {function}: region {region} blocks are not contiguous or not loop-led: {detail}"
    )]
    RegionNotContiguous {
        function: u32,
        region: u32,
        detail: String,
    },
    #[error("function {function}: block {block} is not reachable from the entry block")]
    UnreachableBlock { function: u32, block: u32 },
    #[error(
        "function {function} block {block}: blocks do not tile the instruction arena: {detail}"
    )]
    BlockRangeTile {
        function: u32,
        block: u32,
        detail: String,
    },
    #[error(
        "function {function} block {block} inst {inst}: local {slot} read before a dominating set"
    )]
    LocalNotDominated {
        function: u32,
        block: u32,
        inst: u32,
        slot: u32,
    },
    #[error("function {function}: verifier work ceiling exceeded: {detail}")]
    Internal { function: u32, detail: String },
}

/// Verifies every function of `module`; iterative, no recursion.
pub(crate) fn verify(module: &IrModule) -> Result<(), VerifierError> {
    let function_count =
        u32::try_from(module.functions.len()).map_err(|_| VerifierError::Internal {
            function: u32::MAX,
            detail: String::from("function count overflow"),
        })?;
    for index in 0..function_count {
        let function = module
            .functions
            .get(index as usize)
            .ok_or_else(|| out_of_range(index, 0, 0, String::from("function index")))?;
        verify_function(index, function, module)?;
    }
    Ok(())
}

fn verify_function(
    index: u32,
    function: &IrFunction,
    module: &IrModule,
) -> Result<(), VerifierError> {
    // Rule 7: blocks tile the instruction arena exactly.
    check_tiling(index, &function.blocks, function.insts.len())?;
    // Rules 1, 2, 3: id ranges, operand locality and kind consistency.
    for block_index in 0..function.blocks.len() {
        verify_block(index, function, block_index, module)?;
    }
    // Rule 4: branch discipline.
    check_branch_directions(index, function)?;
    // Rule 5: region contiguity.
    check_regions(index, function)?;
    // Rule 6: reachability from block 0.
    check_reachable(index, function)?;
    // Rule 8: definite-initialization dataflow over an iterative fixpoint.
    check_local_domination(index, function)
}

fn out_of_range(function: u32, block: usize, inst: u32, detail: String) -> VerifierError {
    VerifierError::OutOfRangeIds {
        function,
        block: block as u32,
        inst,
        detail,
    }
}

fn kind_error(function: u32, block: usize, inst: u32, detail: String) -> VerifierError {
    VerifierError::KindMismatch {
        function,
        block: block as u32,
        inst,
        detail,
    }
}

fn tile_error(function: u32, block: usize, detail: String) -> VerifierError {
    VerifierError::BlockRangeTile {
        function,
        block: block as u32,
        detail,
    }
}
