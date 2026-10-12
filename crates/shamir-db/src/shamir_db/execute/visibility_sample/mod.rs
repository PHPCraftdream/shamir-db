//! VIS-4c: sampled settled-prefix position for changefeed-style admin reads.

mod sample;

#[cfg(test)]
mod tests;

pub(super) use sample::changes_since_visible_through;
