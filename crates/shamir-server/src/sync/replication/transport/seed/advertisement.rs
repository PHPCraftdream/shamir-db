use shamir_db::access::{Action, Actor, ResourcePath};
use shamir_db::ShamirDb;
use shamir_query_types::admin::CreatePublicationOp;

use super::fence::publications;
use crate::replication::ReplError;

/// Never partially advertise a publication: doing so could turn a wider scope
/// into an apparently exact receipt match. Omit the whole publication on denial.
pub(crate) async fn authorized_publications(
    home: &ShamirDb,
    actor: &Actor,
) -> Result<Vec<CreatePublicationOp>, ReplError> {
    let mut visible = Vec::new();
    for publication in publications(home).await? {
        let mut authorized = true;
        for scope in &publication.scopes {
            let path = match (&scope.repo, &scope.table) {
                (Some(repo), None) => ResourcePath::store(&scope.db, repo),
                // R2-lite only advertises concrete whole-repository scopes.
                _ => {
                    authorized = false;
                    break;
                }
            };
            if home
                .authorize_access(actor, &path, Action::Read)
                .await
                .is_err()
            {
                authorized = false;
                break;
            }
        }
        if authorized && !publication.scopes.is_empty() {
            visible.push(publication);
        }
    }
    Ok(visible)
}
