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

#[test]
fn rv_tunables_c09f4aa6_interval_conversion_saturates_and_keeps_nonzero() {
    // Regression oracle: 2^64 milliseconds used to truncate to zero via `as u64`.
    let rt = RuntimeTunables::new();
    rt.set_server_poll_interval(Duration::from_micros(900));
    assert_eq!(rt.server_poll_interval(), Duration::from_millis(1));
    let two_to_64_millis = Duration::from_millis(u64::MAX)
        .checked_add(Duration::from_millis(1))
        .expect("2^64 milliseconds fits in Duration");
    rt.set_server_poll_interval(two_to_64_millis);
    assert_eq!(rt.server_poll_interval(), Duration::from_millis(u64::MAX));
    rt.set_server_poll_interval(Duration::ZERO);
    assert_eq!(rt.server_poll_interval(), Duration::ZERO);
}

#[test]
fn rv_tunables_30f8aba8_degenerate_values_round_trip() {
    // Zero capacities are valid stores; consumers clamp them (request_loop `max(1)`).
    let rt = RuntimeTunables::new();
    rt.set_io_frame_buffer_cap(0);
    assert_eq!(rt.io_frame_buffer_cap(), 0);
    rt.set_conn_max_in_flight(0);
    assert_eq!(rt.conn_max_in_flight(), 0);
    rt.set_server_poll_interval(Duration::from_micros(900));
    assert_eq!(rt.server_poll_interval(), Duration::from_millis(1));
}

#[test]
fn rv_tunables_c09f4aa6_interval_conversion_rounds_partial_milliseconds_up() {
    let rt = RuntimeTunables::new();
    rt.set_server_poll_interval(Duration::from_millis(7) + Duration::from_nanos(1));
    assert_eq!(rt.server_poll_interval(), Duration::from_millis(8));
}

#[test]
fn rv_tunables_363b3879_override_is_visible_across_threads() {
    let rt = Arc::new(RuntimeTunables::new());
    let reader = Arc::clone(&rt);
    let writer = std::thread::spawn(move || {
        reader.set_io_frame_buffer_cap(2468);
    });
    writer.join().expect("writer thread should complete");
    assert_eq!(rt.io_frame_buffer_cap(), 2468);
}
