mod metadata;
mod mutation;
mod projection;
mod reference;

pub(in crate::sem::check::builtins) use metadata::record_plan_fact;
pub(in crate::sem::check) use metadata::reference_provenance;
pub(in crate::sem::check::builtins) use mutation::check_batch_mutation as check_mutation_call;
pub(in crate::sem::check::builtins) use projection::{
    check_response_affected, check_row_projection,
};
pub(in crate::sem::check) use reference::{
    check_query_ref_field, check_reference_sink, check_reference_use, finish_reference_local,
    reference_identifier, register_reference_sites, RefLocal, ReferenceSite,
};

#[cfg(test)]
mod tests;
