use super::super::AclMemo;
use crate::access::{Action, Actor, ResourceMeta, ResourcePath, ResourcePathRef};
use crate::shamir_db::ShamirDb;

fn meta(mode: u16) -> ResourceMeta {
    ResourceMeta {
        owner: Actor::System,
        group: None,
        mode,
    }
}

#[test]
fn memo_matches_split_prefix_and_preserves_segment_identity() {
    let memo = AclMemo::default();
    let folder = ResourcePath::function_folder(vec!["a".into(), "".into(), "b".into()]);
    let prefix = ResourcePathRef::FunctionFolderPrefix { prefix: "a//b" };
    let slash_segment = ResourcePath::function_folder(vec!["a/".into(), "b".into()]);
    let empty = ResourcePath::function_folder(vec![]);
    let one_empty = ResourcePathRef::FunctionFolderPrefix { prefix: "" };
    memo.insert_meta(folder.as_ref(), meta(0o111));
    memo.insert_meta(slash_segment.as_ref(), meta(0o222));
    memo.insert_meta(empty.as_ref(), meta(0o333));
    memo.insert_meta(one_empty, meta(0o444));
    assert_eq!(memo.meta(prefix).unwrap().mode, 0o111);
    assert_eq!(memo.meta(slash_segment.as_ref()).unwrap().mode, 0o222);
    assert_eq!(memo.meta(empty.as_ref()).unwrap().mode, 0o333);
    assert_eq!(memo.meta(one_empty).unwrap().mode, 0o444);
    memo.insert_meta(prefix, meta(0o555));
    assert_eq!(memo.meta(folder.as_ref()).unwrap().mode, 0o555);
}

#[test]
fn memo_checks_identity_within_hash_collision_bucket() {
    let memo = AclMemo::default();
    let wanted = ResourcePath::user("a\nb");
    let other = ResourcePath::group("a\nb");
    memo.metas
        .insert(AclMemo::hash(wanted.as_ref()), vec![(other, meta(0o111))]);
    assert!(memo.meta(wanted.as_ref()).is_none());
    memo.insert_meta(wanted.as_ref(), meta(0o222));
    assert_eq!(memo.meta(wanted.as_ref()).unwrap().mode, 0o222);
}

#[tokio::test]
async fn borrowed_meta_matches_owned_for_every_variant() {
    let db = ShamirDb::init_memory().await.unwrap();
    let paths = [
        ResourcePath::Root,
        ResourcePath::database("missing"),
        ResourcePath::store("missing", "s"),
        ResourcePath::table("missing", "s", "t"),
        ResourcePath::record("missing", "s", "t", "k"),
        ResourcePath::index("missing", "s", "t", "i"),
        ResourcePath::FunctionNamespace,
        ResourcePath::WasmCompiler,
        ResourcePath::function_folder(vec!["a".into(), "".into(), "b".into()]),
        ResourcePath::function("a//b/f"),
        ResourcePath::user("u"),
        ResourcePath::group("g"),
    ];
    for path in paths {
        assert_eq!(
            db.resource_meta(&path).await.unwrap(),
            db.resource_meta_ref(path.as_ref()).await.unwrap()
        );
    }
    let folder = ResourcePath::function_folder(vec!["a".into(), "".into(), "b".into()]);
    assert_eq!(
        db.resource_meta(&folder).await.unwrap(),
        db.resource_meta_ref(ResourcePathRef::FunctionFolderPrefix { prefix: "a//b" })
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn first_denial_is_nearest_ancestor_and_path_stays_raw() {
    let db = ShamirDb::init_memory().await.unwrap();
    let memo = AclMemo::default();
    let nearest = ResourcePath::function_folder(vec!["a\nb".into(), "deep".into()]);
    let outer = ResourcePath::function_folder(vec!["a\nb".into()]);
    memo.insert_meta(nearest.as_ref(), meta(0));
    memo.insert_meta(outer.as_ref(), meta(0));
    let path = ResourcePath::function("a\nb/deep/f");
    let denied = db
        .authorize_access_in(&Actor::User(42), &path, Action::Read, Some(&memo))
        .await
        .unwrap_err();
    assert_eq!(denied.path, nearest.to_string());
    assert_eq!(denied.action, Action::Execute);
    assert!(!denied.to_string().contains('\n'));
}
