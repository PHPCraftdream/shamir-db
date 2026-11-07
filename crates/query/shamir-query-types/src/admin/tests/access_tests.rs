use crate::admin::access::ResourceRef;
use shamir_types::access::ResourcePath;

#[cfg(feature = "server")]
#[test]
fn function_folder_ref_round_trip() {
    let r = ResourceRef::FunctionFolder {
        function_folder: vec!["reports".to_string(), "daily".to_string()],
    };
    let bytes = rmp_serde::to_vec_named(&r).expect("serialize");
    let back: ResourceRef = rmp_serde::from_slice(&bytes).expect("deserialize");
    assert_eq!(back, r);

    let path = r.to_path().expect("to_path");
    assert_eq!(
        path,
        ResourcePath::FunctionFolder {
            path: vec!["reports".to_string(), "daily".to_string()],
        }
    );
}
