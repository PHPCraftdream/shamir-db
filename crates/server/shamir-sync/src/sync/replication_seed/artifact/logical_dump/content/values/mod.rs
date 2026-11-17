pub mod conversion;
pub mod record;
pub mod value;

pub use conversion::{from_query_value, into_query_value};

const _: () = {
    let _ = (
        from_query_value
            as fn(&shamir_types::types::value::QueryValue) -> Result<value::LogicalValue, String>,
        into_query_value
            as fn(value::LogicalValue) -> Result<shamir_types::types::value::QueryValue, String>,
    );
};
pub use record::RecordItem;
pub use value::LogicalValue;
