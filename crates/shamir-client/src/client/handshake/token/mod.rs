mod bearer;

#[cfg(test)]
pub(crate) use bearer::decode_binding;
pub(crate) use bearer::read_binding;
