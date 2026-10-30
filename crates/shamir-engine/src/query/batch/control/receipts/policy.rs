use std::borrow::Cow;

use crate::query::batch::{BatchError, BatchOp, BatchRequest, QueryEntry, TableResolver};
use shamir_query_types::batch::VisibilityWait;

/// Preflight the complete tree before dispatch. Transactional DML is covered
/// by the outer receipt; Call/admin writes are independently committed.
pub(in crate::query::batch) async fn validate(
    request: &BatchRequest,
    resolver: &dyn TableResolver,
    root_metadata: Option<&shamir_query_types::batch::RequestMetadata>,
) -> Result<(), BatchError> {
    if !request.transactional
        && !request.queries.iter().any(|(alias, entry)| {
            root_metadata
                .and_then(|metadata| metadata.writes.get(alias).copied())
                .unwrap_or_else(|| entry.op.is_write())
                && !matches!(
                    &entry.op,
                    BatchOp::Insert(_) | BatchOp::Update(_) | BatchOp::Delete(_) | BatchOp::Set(_)
                )
        })
    {
        return Ok(());
    }
    let mut bodies = vec![(request, None)];
    while let Some((body, inherited)) = bodies.pop() {
        let wait = body.visibility_wait.or(inherited);
        let mut bounded = matches!(wait, Some(VisibilityWait::Bounded { .. }));
        let unsupported = body.queries.iter().any(|(alias, entry)| {
            !matches!(
                &entry.op,
                BatchOp::Insert(_)
                    | BatchOp::Update(_)
                    | BatchOp::Delete(_)
                    | BatchOp::Set(_)
                    | BatchOp::Batch(_)
                    | BatchOp::ForEach(_)
            ) && (if std::ptr::eq(body, request) {
                root_metadata.and_then(|metadata| metadata.writes.get(alias).copied())
            } else {
                None
            })
            .unwrap_or_else(|| entry.op.is_write())
        });
        if wait.is_none() && unsupported {
            let repos = if std::ptr::eq(body, request) {
                root_metadata.map(|metadata| metadata.repos.clone())
            } else {
                None
            }
            .unwrap_or_else(|| shamir_query_types::batch::distinct_repos(&body.queries));
            for repo_name in repos {
                // An unresolvable repository has no commit policy to inherit:
                // execution reports the real error where it always did.
                if let Ok(repo) = resolver.resolve_repo(&repo_name).await {
                    bounded |= matches!(
                        repo.commit_visibility_wait(),
                        VisibilityWait::Bounded { .. }
                    );
                }
            }
        }
        for entry in body.queries.values() {
            if let BatchOp::Call(call) = &entry.op {
                if wait.is_none() {
                    if let Ok(repo) = resolver.resolve_repo(&call.repo).await {
                        bounded |= matches!(
                            repo.commit_visibility_wait(),
                            VisibilityWait::Bounded { .. }
                        );
                    }
                }
            }
        }
        for (alias, entry) in &body.queries {
            let nested = match &entry.op {
                BatchOp::Batch(sub) => Some(&sub.batch),
                BatchOp::ForEach(fe) => Some(&fe.batch),
                _ => None,
            };
            if let Some(nested) = nested {
                if body.transactional
                    && matches!(nested.visibility_wait, Some(VisibilityWait::Bounded { .. }))
                {
                    return Err(BatchError::query_coded(
                        alias,
                        "validation",
                        "nested commit policy cannot override an open outer transaction",
                    ));
                }
                bodies.push((nested, wait));
                continue;
            }
            if bounded
                && entry.op.is_write()
                && !matches!(
                    &entry.op,
                    BatchOp::Insert(_) | BatchOp::Update(_) | BatchOp::Delete(_) | BatchOp::Set(_)
                )
            {
                return Err(BatchError::query_coded(
                    alias,
                    "validation",
                    "bounded visibility is unsupported by independently committed Call/admin writes",
                ));
            }
        }
    }
    Ok(())
}

/// Clone only a container requiring policy inheritance, never a leaf hot path.
pub(in crate::query::batch) fn inherit(
    entry: &QueryEntry,
    wait: Option<VisibilityWait>,
) -> Cow<'_, QueryEntry> {
    let missing = match &entry.op {
        BatchOp::Batch(sub) => sub.batch.visibility_wait.is_none(),
        BatchOp::ForEach(fe) => fe.batch.visibility_wait.is_none(),
        _ => false,
    };
    if !missing || wait.is_none() {
        return Cow::Borrowed(entry);
    }
    let mut entry = entry.clone();
    match &mut entry.op {
        BatchOp::Batch(sub) => sub.batch.visibility_wait = wait,
        BatchOp::ForEach(fe) => fe.batch.visibility_wait = wait,
        _ => {}
    }
    Cow::Owned(entry)
}
