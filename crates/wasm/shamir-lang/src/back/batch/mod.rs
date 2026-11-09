mod lower;
mod template;

pub(crate) use lower::{is_batch_builtin, lower_batch_builtin};
pub(crate) use template::BatchStatics;
