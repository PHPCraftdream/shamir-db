//! Token-authenticated REST gate and executor.

mod bucket;
mod executor;
mod gate;
mod maintenance;
mod scope;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use scope::AllowedOperation;

pub(crate) use executor::HandleDenial;
/// Constructor for T3b-3 listener wiring, plus transport trait implementations.
pub use executor::{RestHandler, ScopedHandler};
pub(crate) use gate::BrowserVerdict;
/// Gate and fixed listener security rails for T3b-3 wiring.
pub use gate::{RestGate, RestGateRails};
