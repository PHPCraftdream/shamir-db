use serde::{Deserialize, Serialize};

use crate::wire::repl::{ReplRepoInfo, ReplResponse};

#[derive(Serialize, Deserialize)]
struct OldRepo {
    db: String,
    repo: String,
    current_version: u64,
    journal_floor: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "repl_kind", rename_all = "snake_case")]
enum OldResponse {
    Hello {
        leader_epoch: u64,
        repos: Vec<OldRepo>,
    },
    Pull {
        leader_epoch: u64,
        #[serde(with = "serde_bytes")]
        events: Vec<u8>,
        gap_at: Option<u64>,
        current_version: u64,
    },
}

#[test]
fn old_shape_hello_and_pull_decode_absent_identity() {
    // Prior wire shapes deliberately bypass query builders.
    let bytes = rmp_serde::to_vec_named(&OldResponse::Hello {
        leader_epoch: 1,
        repos: vec![OldRepo {
            db: "app".into(),
            repo: "main".into(),
            current_version: 100,
            journal_floor: 0,
        }],
    })
    .unwrap();
    let hello: ReplResponse = rmp_serde::from_slice(&bytes).unwrap();
    let ReplResponse::Hello { repos, .. } = hello else {
        panic!("expected Hello");
    };
    assert_eq!(repos[0].repo_lineage_id, None);
    assert_eq!(repos[0].history_epoch, None);
    assert_eq!((repos[0].gap_first, repos[0].gap_through), (None, None));
    let bytes = rmp_serde::to_vec_named(&OldResponse::Pull {
        leader_epoch: 1,
        events: vec![],
        gap_at: Some(10),
        current_version: 100,
    })
    .unwrap();
    let pull: ReplResponse = rmp_serde::from_slice(&bytes).unwrap();
    let ReplResponse::Pull {
        repo_lineage_id,
        history_epoch,
        gap_first,
        gap_through,
        ..
    } = pull
    else {
        panic!("expected Pull");
    };
    assert_eq!(
        (repo_lineage_id, history_epoch, gap_first, gap_through),
        (None, None, None, None),
    );
}

#[test]
fn identity_interval_named_messagepack_roundtrip() {
    for epoch in [0, 1, u64::MAX] {
        let hello = ReplResponse::Hello {
            leader_epoch: 1,
            repos: vec![ReplRepoInfo {
                db: "app".into(),
                repo: "main".into(),
                current_version: 100,
                journal_floor: 0,
                repo_lineage_id: Some([255; 16]),
                history_epoch: Some(epoch),
                gap_first: Some(10),
                gap_through: Some(100),
            }],
        };
        let pull = ReplResponse::Pull {
            leader_epoch: 1,
            events: vec![],
            gap_at: Some(10),
            current_version: 100,
            repo_lineage_id: Some([255; 16]),
            history_epoch: Some(epoch),
            gap_first: Some(10),
            gap_through: Some(100),
        };
        for response in [hello, pull] {
            let bytes = rmp_serde::to_vec_named(&response).unwrap();
            assert_eq!(
                rmp_serde::from_slice::<ReplResponse>(&bytes).unwrap(),
                response
            );
            let old: OldResponse = rmp_serde::from_slice(&bytes).unwrap();
            match (old, &response) {
                (
                    OldResponse::Hello {
                        leader_epoch,
                        repos,
                    },
                    ReplResponse::Hello { .. },
                ) => {
                    assert_eq!(leader_epoch, 1);
                    assert_eq!(repos.len(), 1);
                    assert_eq!(repos[0].db, "app");
                    assert_eq!(repos[0].repo, "main");
                    assert_eq!(repos[0].current_version, 100);
                    assert_eq!(repos[0].journal_floor, 0);
                }
                (
                    OldResponse::Pull {
                        leader_epoch,
                        events,
                        gap_at,
                        current_version,
                    },
                    ReplResponse::Pull { .. },
                ) => {
                    assert_eq!(leader_epoch, 1);
                    assert_eq!(events, Vec::<u8>::new());
                    assert_eq!(gap_at, Some(10));
                    assert_eq!(current_version, 100);
                }
                _ => panic!("old response kind must match new response"),
            }
        }
    }
}
