//! Histogram bucket layout for the HTTP login duration metrics.

use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};

use super::ObservabilityError;

/// Fixed upper bounds (seconds) around the 50-75 ms policy padding, up to the request deadline.
const BUCKETS: &[f64] = &[
    0.05, 0.055, 0.06, 0.065, 0.07, 0.075, 0.1, 0.25, 0.5, 1.0, 5.0, 120.0,
];

/// A builder with the login BEGIN and FINISH duration histograms configured.
pub(super) fn builder_with_login_buckets() -> Result<PrometheusBuilder, ObservabilityError> {
    let mut builder = PrometheusBuilder::new();
    for metric in [
        "shamir_login_http_begin_duration_seconds",
        "shamir_login_http_finish_duration_seconds",
    ] {
        builder = builder
            .set_buckets_for_metric(Matcher::Full(metric.into()), BUCKETS)
            .map_err(|error| ObservabilityError::RecorderInstall(error.to_string()))?;
    }
    Ok(builder)
}
