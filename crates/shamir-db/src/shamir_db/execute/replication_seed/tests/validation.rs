use shamir_engine::meta::repo_identity::RepoIdentity;
use shamir_engine::repo::SeedReceipt;
use shamir_query_types::admin::{CreatePublicationOp, ReplScope};
use shamir_query_types::wire::repl::{ReplRepoInfo, ReplResponse};

use crate::{validate_seed_hello, SeedRefusal};

pub(super) fn receipt() -> SeedReceipt {
    SeedReceipt {
        artifact_digest: "a".repeat(64),
        source_db: "app".into(),
        source_repo: "main".into(),
        source_identity: RepoIdentity {
            lineage_id: [1; 16],
            history_epoch: 2,
        },
        schema_digest: "b".repeat(64),
        source_version: 10,
        local_baseline: 1,
    }
}

pub(super) fn hello() -> ReplResponse {
    ReplResponse::Hello {
        leader_epoch: 1,
        repos: vec![ReplRepoInfo {
            db: "app".into(),
            repo: "main".into(),
            current_version: 10,
            journal_floor: 0,
            repo_lineage_id: Some([1; 16]),
            history_epoch: Some(2),
            gap_first: Some(1),
            gap_through: Some(10),
            schema_digest: Some("b".repeat(64)),
            settled_version: Some(10),
            publications: vec![CreatePublicationOp {
                create_publication: "pub".into(),
                scopes: vec![ReplScope {
                    db: "app".into(),
                    repo: Some("main".into()),
                    table: None,
                }],
            }],
        }],
    }
}

/// Before SEED-4 Resume accepted all these peers without observing Hello.
#[test]
fn resume_metadata_refusals_are_distinct_and_matching_gap_is_not_bypassed() {
    let cases: &[(fn(&mut ReplRepoInfo), SeedRefusal)] = &[
        (|r| r.repo_lineage_id = None, SeedRefusal::MissingIdentity),
        (
            |r| r.repo_lineage_id = Some([2; 16]),
            SeedRefusal::WrongIdentity,
        ),
        (
            |r| r.history_epoch = Some(3),
            SeedRefusal::WrongHistoryEpoch,
        ),
        (
            |r| r.schema_digest = Some("c".repeat(64)),
            SeedRefusal::WrongSchema,
        ),
        (|r| r.publications.clear(), SeedRefusal::WrongPublication),
        (
            |r| r.publications[0].scopes[0].table = Some("rows".into()),
            SeedRefusal::WrongScope,
        ),
        (
            |r| r.gap_through = Some(11),
            SeedRefusal::HorizonAboveBookmark,
        ),
        (
            |r| r.settled_version = Some(9),
            SeedRefusal::SourceRegression,
        ),
    ];
    for (mutate, expected) in cases {
        let mut response = hello();
        if let ReplResponse::Hello { repos, .. } = &mut response {
            mutate(&mut repos[0]);
        }
        assert_eq!(
            validate_seed_hello(&receipt(), 10, "pub", &response),
            Err(*expected)
        );
    }
    assert_eq!(
        validate_seed_hello(&receipt(), u64::MAX, "pub", &hello()),
        Err(SeedRefusal::BookmarkOverflow)
    );
    assert_eq!(validate_seed_hello(&receipt(), 10, "pub", &hello()), Ok(()));
    assert_eq!(receipt().source_version.checked_add(1), Some(11));
}
