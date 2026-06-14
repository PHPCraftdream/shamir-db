use shamir_collections::THasher;
use std::collections::HashMap;

#[test]
fn watermark_skips_already_seen_versions() {
    let mut watermarks: HashMap<String, u64, THasher> = HashMap::<_, _, THasher>::default();
    let repo = "repo_a".to_string();

    watermarks.insert(repo.clone(), 5);

    let wm = watermarks.entry(repo.clone()).or_insert(0);
    assert!(3 <= *wm, "version 3 should be skipped (watermark=5)");
    assert!(5 <= *wm, "version 5 should be skipped (watermark=5)");

    assert!(6 > *wm, "version 6 should pass (watermark=5)");
    *wm = 6;
    assert_eq!(watermarks[&repo], 6);
}

#[test]
fn watermark_independent_per_repo() {
    let mut watermarks: HashMap<String, u64, THasher> = HashMap::<_, _, THasher>::default();
    watermarks.insert("repo_a".to_string(), 10);
    watermarks.insert("repo_b".to_string(), 3);

    let wm_a = watermarks.entry("repo_a".to_string()).or_insert(0);
    assert!(
        5 <= *wm_a,
        "repo_a version 5 should be skipped (watermark=10)"
    );

    let wm_b = watermarks.entry("repo_b".to_string()).or_insert(0);
    assert!(5 > *wm_b, "repo_b version 5 should pass (watermark=3)");
    *wm_b = 5;

    assert_eq!(watermarks["repo_a"], 10);
    assert_eq!(watermarks["repo_b"], 5);
}

#[test]
fn watermark_backfill_tracks_max_version() {
    let mut watermarks: HashMap<String, u64, THasher> = HashMap::<_, _, THasher>::default();
    let repo = "repo_a".to_string();

    for version in [2, 5, 3, 7, 6] {
        let wm = watermarks.entry(repo.clone()).or_insert(0);
        if version > *wm {
            *wm = version;
        }
    }

    assert_eq!(watermarks[&repo], 7);
}
