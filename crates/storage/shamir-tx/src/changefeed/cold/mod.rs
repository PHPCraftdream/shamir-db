//! Cold-start durable changefeed journal operations.

mod journal;

#[cfg(test)]
mod tests;

pub use journal::{
    delete_event, delete_events_in_range, encode_event, events_in_range, flush_journal,
    put_event_if_missing, read_event,
};
