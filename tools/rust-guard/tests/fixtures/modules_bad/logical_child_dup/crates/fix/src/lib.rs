#[cfg(unix)]
#[path = "shared.rs"]
mod a;
#[cfg(not(unix))]
#[path = "shared.rs"]
mod b;
#[cfg(not(unix))]
#[path = "shared/child.rs"]
mod direct;
