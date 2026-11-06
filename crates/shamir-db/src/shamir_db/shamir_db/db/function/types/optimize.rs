use shamir_engine::function::TierMode;

/// Stable storage/wire name of a tier mode.
pub fn optimize_name(mode: TierMode) -> &'static str {
    match mode {
        TierMode::Auto => "auto",
        TierMode::Now => "now",
        TierMode::Never => "never",
    }
}

/// Inverse of [`optimize_name`]; `None` for an unknown string.
pub fn parse_optimize(s: &str) -> Option<TierMode> {
    match s {
        "auto" => Some(TierMode::Auto),
        "now" => Some(TierMode::Now),
        "never" => Some(TierMode::Never),
        _ => None,
    }
}
