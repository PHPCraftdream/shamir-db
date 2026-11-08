use crate::api::{Diagnostic, Position, Span};
use crate::front::CompileBudget;

const CHECKPOINT_STRIDE: usize = 16;

#[derive(Clone, Debug)]
pub(super) struct LineIndex {
    checkpoints: Vec<Checkpoint>,
}

#[derive(Clone, Copy, Debug)]
struct Checkpoint {
    byte: usize,
    position: Position,
    previous_cr: bool,
}

impl LineIndex {
    pub(super) fn build(source: &str, budget: &mut CompileBudget) -> Result<Self, Diagnostic> {
        let mut checkpoints = vec![Checkpoint {
            byte: 0,
            position: Position { line: 1, column: 1 },
            previous_cr: false,
        }];
        let mut offset = 0usize;
        let mut position = Position { line: 1, column: 1 };
        let mut previous_cr = false;
        let mut next_checkpoint = CHECKPOINT_STRIDE;
        for character in source.chars() {
            if budget
                .charge(character.len_utf8() as u64, Span::source_start())
                .is_err()
            {
                return Err(fuel_error(offset, position, budget.limit()));
            }
            if offset >= next_checkpoint {
                checkpoints.push(Checkpoint {
                    byte: offset,
                    position,
                    previous_cr,
                });
                next_checkpoint = offset.saturating_add(CHECKPOINT_STRIDE);
            }
            if character == '\r' {
                position.line = position.line.saturating_add(1);
                position.column = 1;
                previous_cr = true;
            } else if character == '\n' {
                if !previous_cr {
                    position.line = position.line.saturating_add(1);
                }
                position.column = 1;
                previous_cr = false;
            } else {
                position.column = position.column.saturating_add(1);
                previous_cr = false;
            }
            offset += character.len_utf8();
        }
        Ok(Self { checkpoints })
    }

    pub(super) fn position_at_metered(
        &self,
        source: &str,
        offset: usize,
        budget: &mut CompileBudget,
    ) -> Result<Position, Diagnostic> {
        let search_bound = usize::BITS - (self.checkpoints.len().saturating_sub(1)).leading_zeros();
        if budget
            .charge(u64::from(search_bound), Span::source_start())
            .is_err()
        {
            return Err(fuel_error(
                offset,
                self.position_at(source, offset),
                budget.limit(),
            ));
        }
        let checkpoint = self.checkpoint_at(offset);
        let work = u64::try_from(offset.saturating_sub(checkpoint.byte)).unwrap_or(u64::MAX);
        if budget.charge(work, Span::source_start()).is_err() {
            return Err(fuel_error(
                offset,
                self.position_at(source, offset),
                budget.limit(),
            ));
        }
        Ok(position_from_checkpoint(source, offset, checkpoint))
    }

    pub(super) fn position_at(&self, source: &str, offset: usize) -> Position {
        position_from_checkpoint(source, offset, self.checkpoint_at(offset))
    }

    fn checkpoint_at(&self, offset: usize) -> Checkpoint {
        let mut low = 0usize;
        let mut high = self.checkpoints.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if self
                .checkpoints
                .get(middle)
                .is_some_and(|point| point.byte <= offset)
            {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        self.checkpoints
            .get(low.saturating_sub(1))
            .copied()
            .unwrap_or(Checkpoint {
                byte: 0,
                position: Position { line: 1, column: 1 },
                previous_cr: false,
            })
    }
}

fn position_from_checkpoint(source: &str, offset: usize, checkpoint: Checkpoint) -> Position {
    let mut position = checkpoint.position;
    let mut previous_cr = checkpoint.previous_cr;
    let remaining = source.get(checkpoint.byte..offset);
    if let Some(remaining) = remaining {
        for character in remaining.chars() {
            if character == '\r' {
                position.line = position.line.saturating_add(1);
                position.column = 1;
                previous_cr = true;
            } else if character == '\n' {
                if !previous_cr {
                    position.line = position.line.saturating_add(1);
                }
                position.column = 1;
                previous_cr = false;
            } else {
                position.column = position.column.saturating_add(1);
                previous_cr = false;
            }
        }
    }
    position
}

pub(super) fn boundary_at_or_before(source: &str, mut offset: usize) -> usize {
    while !source.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    offset
}

pub(super) fn boundary_at_or_after(source: &str, mut offset: usize) -> usize {
    while !source.is_char_boundary(offset) && offset < source.len() {
        offset += 1;
    }
    offset
}

fn point_span(offset: usize, position: Position) -> Span {
    let byte = u32::try_from(offset).unwrap_or(u32::MAX);
    Span::new(byte, byte, position, position)
}

fn fuel_error(offset: usize, position: Position, limit: u64) -> Diagnostic {
    Diagnostic::new(
        "B001",
        format!("compile fuel exceeded (limit {})", limit),
        point_span(offset, position),
    )
}
