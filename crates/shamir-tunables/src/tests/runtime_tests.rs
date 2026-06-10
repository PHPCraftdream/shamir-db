use std::sync::Arc;
use std::time::Duration;

use crate::instance_defaults;
use crate::runtime::RuntimeTunables;

#[test]
fn defaults_equal_consts() {
    let rt = RuntimeTunables::new();
    assert_eq!(
        rt.io_frame_buffer_cap(),
        instance_defaults::IO_FRAME_BUFFER_CAP,
        "io_frame_buffer_cap default must match instance_defaults const"
    );
    assert_eq!(
        rt.server_poll_interval(),
        instance_defaults::SERVER_POLL_INTERVAL,
        "server_poll_interval default must match instance_defaults const"
    );
    assert_eq!(
        rt.conn_max_in_flight(),
        instance_defaults::CONN_MAX_IN_FLIGHT,
        "conn_max_in_flight default must match instance_defaults const"
    );
}

#[test]
fn set_io_frame_buffer_cap_then_read() {
    let rt = RuntimeTunables::new();
    rt.set_io_frame_buffer_cap(8192);
    assert_eq!(rt.io_frame_buffer_cap(), 8192);
}

#[test]
fn set_server_poll_interval_then_read() {
    let rt = RuntimeTunables::new();
    let new_interval = Duration::from_millis(100);
    rt.set_server_poll_interval(new_interval);
    assert_eq!(rt.server_poll_interval(), new_interval);
}

#[test]
fn set_conn_max_in_flight_then_read() {
    let rt = RuntimeTunables::new();
    rt.set_conn_max_in_flight(8);
    assert_eq!(rt.conn_max_in_flight(), 8);
}

/// Reads take `&self` — the struct is shareable via `Arc<RuntimeTunables>`.
/// This test proves the API is callable on a shared reference.
#[test]
fn reads_are_shared_ref() {
    let rt = Arc::new(RuntimeTunables::new());
    let rt2 = rt.clone();
    // Both reads and writes work through a shared Arc<>.
    rt2.set_io_frame_buffer_cap(1234);
    assert_eq!(rt.io_frame_buffer_cap(), 1234);
}
