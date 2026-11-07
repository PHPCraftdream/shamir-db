use std::collections::hash_map::DefaultHasher;
use std::hash::{BuildHasherDefault, Hash, Hasher};

use indexmap::IndexSet;

use crate::access::{ResourcePath, ResourcePathRef};

#[derive(Default)]
struct ConstantHasher;

impl Hasher for ConstantHasher {
    fn finish(&self) -> u64 {
        0
    }
    fn write(&mut self, _: &[u8]) {}
}

fn path_hash(path: ResourcePathRef<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

fn folder(parts: &[&str]) -> ResourcePath {
    ResourcePath::function_folder(parts.iter().map(|part| (*part).to_owned()).collect())
}

#[test]
fn borrowed_ancestors_match_independent_contract_for_every_variant() {
    let root = ResourcePath::Root;
    let namespace = ResourcePath::FunctionNamespace;
    let db = ResourcePath::database("ד");
    let store = ResourcePath::store("ד", "s");
    let table = ResourcePath::table("ד", "s", "t");
    let cases = vec![
        (root.clone(), vec![]),
        (db.clone(), vec![root.clone()]),
        (store.clone(), vec![db.clone(), root.clone()]),
        (table.clone(), vec![store.clone(), db.clone(), root.clone()]),
        (
            ResourcePath::record("ד", "s", "t", "k"),
            vec![table.clone(), store.clone(), db.clone(), root.clone()],
        ),
        (
            ResourcePath::index("ד", "s", "t", "i"),
            vec![table, store, db, root.clone()],
        ),
        (namespace.clone(), vec![root.clone()]),
        (ResourcePath::WasmCompiler, vec![root.clone()]),
        (folder(&[]), vec![namespace.clone(), root.clone()]),
        (folder(&["a"]), vec![namespace.clone(), root.clone()]),
        (
            folder(&["a", "", "b"]),
            vec![
                folder(&["a", ""]),
                folder(&["a"]),
                namespace.clone(),
                root.clone(),
            ],
        ),
        (
            ResourcePath::function("f"),
            vec![namespace.clone(), root.clone()],
        ),
        (ResourcePath::user("u"), vec![root.clone()]),
        (ResourcePath::group("g"), vec![root]),
    ];
    for (path, expected) in cases {
        assert_eq!(path.as_ref().to_owned(), path);
        let actual: Vec<_> = path
            .as_ref()
            .ancestors()
            .map(ResourcePathRef::to_owned)
            .collect();
        assert_eq!(actual, expected, "{path:?}");
        assert_eq!(path.parent(), expected.first().cloned());
        assert_eq!(path.ancestors(), expected);
    }
}

#[test]
fn folder_views_have_canonical_equality_and_hashes() {
    for prefix in ["", "/", "a/", "/a", "é//x/", "a/b"] {
        let parts: Vec<String> = prefix.split('/').map(str::to_owned).collect();
        let slice = ResourcePathRef::FunctionFolder { path: &parts };
        let split = ResourcePathRef::FunctionFolderPrefix { prefix };
        assert_eq!(slice, split);
        assert_eq!(split, slice);
        assert_eq!(path_hash(slice), path_hash(split));
        assert_eq!(slice.to_owned(), split.to_owned());
    }
}

#[test]
fn folder_identity_survives_hash_collisions() {
    let empty: Vec<String> = vec![];
    let one_empty = vec![String::new()];
    let slash = vec!["a/b".to_owned()];
    let split = vec!["a".to_owned(), "b".to_owned()];
    let paths = [
        ResourcePathRef::FunctionFolder { path: &empty },
        ResourcePathRef::FunctionFolder { path: &one_empty },
        ResourcePathRef::FunctionFolder { path: &slash },
        ResourcePathRef::FunctionFolder { path: &split },
    ];
    let mut set: IndexSet<_, BuildHasherDefault<ConstantHasher>> = IndexSet::default();
    set.extend(paths);
    assert_eq!(set.len(), 4);
    assert!(set.contains(&ResourcePathRef::FunctionFolderPrefix { prefix: "" }));
    assert!(set.contains(&ResourcePathRef::FunctionFolderPrefix { prefix: "a/b" }));
    assert_ne!(
        paths[0],
        ResourcePathRef::FunctionFolderPrefix { prefix: "" }
    );
    assert_ne!(
        paths[2],
        ResourcePathRef::FunctionFolderPrefix { prefix: "a/b" }
    );
    assert_ne!(paths[2], paths[3]);
}

#[test]
fn display_and_owned_conversion_preserve_folder_identity() {
    for parts in [&[][..], &["a"][..], &["a", "", "b"][..]] {
        let path = folder(parts);
        let view = path.as_ref();
        assert_eq!(view.to_owned(), path);
        assert_eq!(format!("{view}"), format!("{path}"));
        assert_eq!(path_hash(view), path_hash(path.as_ref()));
    }
}

#[test]
fn borrowed_folder_canonical_path_matches_segments() {
    // Production path: a function's nearest folder ancestor is the borrowed
    // slash-joined prefix; it must equal, display, convert and hash exactly
    // like the owned folder built from the same segments.
    let path = ResourcePath::function("tenant/reports/daily");
    let nearest = path.as_ref().ancestors().next().expect("nearest ancestor");
    assert_eq!(
        nearest,
        ResourcePathRef::FunctionFolderPrefix {
            prefix: "tenant/reports"
        }
    );
    let owned = folder(&["tenant", "reports"]);
    assert_eq!(nearest, owned.as_ref());
    assert_eq!(nearest.to_owned(), owned);
    assert_eq!(format!("{nearest}"), format!("{owned}"));
    assert_eq!(path_hash(nearest), path_hash(owned.as_ref()));
}

#[test]
fn borrowed_display_matches_owned_for_all_variants_and_controls() {
    let parts = vec!["a/b".to_owned(), "\n\0é".to_owned(), String::new()];
    let empty: Vec<String> = vec![];
    let value = "x\n\r\0\té/";
    let cases = [
        ResourcePathRef::Root,
        ResourcePathRef::Database { db: value },
        ResourcePathRef::Store {
            db: value,
            store: value,
        },
        ResourcePathRef::Table {
            db: value,
            store: value,
            table: value,
        },
        ResourcePathRef::Record {
            db: value,
            store: value,
            table: value,
            key: value,
        },
        ResourcePathRef::Index {
            db: value,
            store: value,
            table: value,
            index: value,
        },
        ResourcePathRef::FunctionNamespace,
        ResourcePathRef::WasmCompiler,
        ResourcePathRef::FunctionFolder { path: &parts },
        ResourcePathRef::FunctionFolder { path: &empty },
        ResourcePathRef::FunctionFolderPrefix { prefix: value },
        ResourcePathRef::FunctionFolderPrefix { prefix: "" },
        ResourcePathRef::Function { name: value },
        ResourcePathRef::User { name: value },
        ResourcePathRef::Group { name: value },
    ];
    for path in cases {
        assert_eq!(path.to_string(), path.to_owned().to_string(), "{path:?}");
    }
}

#[test]
fn function_prefixes_preserve_empty_segments_and_unicode() {
    let cases = [
        ("", vec![]),
        ("/", vec![folder(&[""])]),
        ("a/", vec![folder(&["a"])]),
        ("/a", vec![folder(&[""])]),
        (
            "é//x/",
            vec![folder(&["é", "", "x"]), folder(&["é", ""]), folder(&["é"])],
        ),
        ("//", vec![folder(&["", ""]), folder(&[""])]),
    ];
    for (name, mut expected) in cases {
        expected.extend([ResourcePath::FunctionNamespace, ResourcePath::Root]);
        let path = ResourcePath::function(name);
        let actual: Vec<_> = path
            .as_ref()
            .ancestors()
            .map(ResourcePathRef::to_owned)
            .collect();
        assert_eq!(actual, expected, "{name:?}");
        assert_eq!(path.ancestors(), expected);
    }
}
