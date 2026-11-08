/// Arena index of one instruction inside an `IrFunction`'s flat `insts` vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InstId(pub u32);

/// Arena index of one basic block inside an `IrFunction`'s `blocks` vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);
