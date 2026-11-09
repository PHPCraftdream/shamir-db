pub(crate) use shamir_sync::durability::maintenance_lock::acquire;

pub(crate) fn acquire_server(
    data_dir: &std::path::Path,
) -> Result<std::fs::File, crate::server::BootError> {
    shamir_sync::durability::maintenance_lock::acquire_with(data_dir, |_| {
        crate::server::BootError::AlreadyRunning(data_dir.to_path_buf())
    })
}
