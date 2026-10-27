use crate::admin::{CreateFunctionOp, DropDbOp, DropTableOp, GroupRef, ResourceRef};
use crate::auth::SecretString;
use crate::batch::BatchOp;
use crate::hmac::{
    canonical_batch_op, compute_tag_hex, derive_session_hmac_key, hmac_policy, verify_tag_hex,
    HmacPolicy,
};
use crate::wire::DbRequest;
use hmac::{Hmac, Mac};
use sha2::Sha256;
#[test]
fn key_derivation_stays_domain_separated() {
    let a = derive_session_hmac_key(&[7; 32]);
    assert_eq!(a, derive_session_hmac_key(&[7; 32]));
    assert_ne!(a, derive_session_hmac_key(&[8; 32]));
}
#[test]
fn tag_verify_keeps_constant_time_verifier_and_rejects_tampering() {
    let k = derive_session_hmac_key(&[1; 32]);
    let bytes = b"v2 vector";
    let tag = compute_tag_hex(&k, bytes);
    assert!(verify_tag_hex(&k, bytes, &tag));
    assert!(!verify_tag_hex(&k, b"changed", &tag));
    assert!(!verify_tag_hex(&k, bytes, "bad"));
    assert!(!verify_tag_hex(
        &derive_session_hmac_key(&[2; 32]),
        bytes,
        &tag
    ));
}
#[test]
fn rv_query_types_9cde03ef_flags_are_bound_and_v1_rejected() {
    let create_user = |roles| {
        BatchOp::CreateUser(crate::auth::CreateUserOp {
            create_user: "u".into(),
            password: SecretString::from("pw".to_owned()),
            roles,
            database: None,
            hmac: None,
        })
    };
    let no_roles = create_user(vec![]);
    let one_role = create_user(vec!["r".into()]);
    assert_ne!(
        canonical_batch_op("d", &no_roles).unwrap(),
        canonical_batch_op("d", &one_role).unwrap()
    );
    let mk = |cascade| {
        BatchOp::DropDb(DropDbOp {
            drop_db: "db".into(),
            hmac: None,
            cascade,
            if_exists: false,
        })
    };
    let a = canonical_batch_op("", &mk(false)).unwrap();
    let b = canonical_batch_op("", &mk(true)).unwrap();
    assert_ne!(a, b);
    let key = derive_session_hmac_key(&[1; 32]);
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(&key).unwrap();
    mac.update(b"drop_db\0db");
    let v1 = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert!(!verify_tag_hex(&key, &b, &v1));
}
#[test]
fn rv_query_types_c7aec31f_separator_and_resource_aliases_are_distinct() {
    let mk = |r: &str, u: &str| {
        BatchOp::GrantRole(crate::auth::GrantRoleOp {
            grant_role: r.into(),
            user: u.into(),
            hmac: None,
        })
    };
    assert_ne!(
        canonical_batch_op("", &mk("a\0b", "c")),
        canonical_batch_op("", &mk("a", "b\0c"))
    );
    let function_with_grants = |secret_grants| {
        BatchOp::CreateFunction(CreateFunctionOp {
            create_function: "f".into(),
            source: None,
            wasm: None,
            replace: false,
            visibility: None,
            security: None,
            secret_grants,
            net_grants: vec![],
            hmac: None,
        })
    };
    assert_ne!(
        canonical_batch_op("", &function_with_grants(vec!["a,b".into()])).unwrap(),
        canonical_batch_op("", &function_with_grants(vec!["a".into(), "b".into()])).unwrap()
    );

    let store = BatchOp::Chmod(crate::admin::ChmodOp {
        chmod: ResourceRef::Store {
            store: ["a".into(), "b/c".into()],
        },
        mode: 0,
        hmac: None,
    });
    let table = BatchOp::Chmod(crate::admin::ChmodOp {
        chmod: ResourceRef::Table {
            table: ["a".into(), "b".into(), "c".into()],
        },
        mode: 0,
        hmac: None,
    });
    let store_bytes = canonical_batch_op("", &store).unwrap();
    let table_bytes = canonical_batch_op("", &table).unwrap();
    assert_ne!(store_bytes, table_bytes);
    let key = derive_session_hmac_key(&[0x31; 32]);
    assert_ne!(
        compute_tag_hex(&key, &store_bytes),
        compute_tag_hex(&key, &table_bytes)
    );
    let f = ResourceRef::Function {
        function: "".into(),
    };
    let ns = ResourceRef::FunctionNamespace {
        function_namespace: true,
    };
    let function = BatchOp::Chmod(crate::admin::ChmodOp {
        chmod: f,
        mode: 0,
        hmac: None,
    });
    let namespace = BatchOp::Chmod(crate::admin::ChmodOp {
        chmod: ns,
        mode: 0,
        hmac: None,
    });
    assert_ne!(
        canonical_batch_op("", &function).unwrap(),
        canonical_batch_op("", &namespace).unwrap()
    );
    let named_folder = BatchOp::Chmod(crate::admin::ChmodOp {
        chmod: ResourceRef::FunctionFolder {
            function_folder: vec!["a/b".into()],
        },
        mode: 0,
        hmac: None,
    });
    let split_folder = BatchOp::Chmod(crate::admin::ChmodOp {
        chmod: ResourceRef::FunctionFolder {
            function_folder: vec!["a".into(), "b".into()],
        },
        mode: 0,
        hmac: None,
    });
    assert_ne!(
        canonical_batch_op("", &named_folder).unwrap(),
        canonical_batch_op("", &split_folder).unwrap()
    );
    let named_group = BatchOp::DropGroup(crate::admin::DropGroupOp {
        drop_group: GroupRef::Name {
            name: "id:3".into(),
        },
        if_exists: false,
        hmac: None,
    });
    let id_group = BatchOp::DropGroup(crate::admin::DropGroupOp {
        drop_group: GroupRef::Id { id: 3 },
        if_exists: false,
        hmac: None,
    });
    assert_ne!(
        canonical_batch_op("", &named_group).unwrap(),
        canonical_batch_op("", &id_group).unwrap()
    );
}
#[test]
fn rv_query_types_c7aec31f_scram_lists_and_resource_shapes_are_distinct() {
    let scram = |name: &str, roles: Vec<String>| DbRequest::CreateScramUser {
        name: name.into(),
        password: crate::auth::SecretString::from("password".to_owned()),
        roles,
        hmac: None,
    };
    assert_ne!(
        crate::hmac::canonical_db_request(&scram("a\0b", vec!["c".into()])).unwrap(),
        crate::hmac::canonical_db_request(&scram("a", vec!["b\0c".into()])).unwrap()
    );
    assert_ne!(
        crate::hmac::canonical_db_request(&scram("u", vec!["a".into(), "b".into()])).unwrap(),
        crate::hmac::canonical_db_request(&scram("u", vec!["a,b".into()])).unwrap()
    );

    let chmod = |resource| {
        BatchOp::Chmod(crate::admin::ChmodOp {
            chmod: resource,
            mode: 0,
            hmac: None,
        })
    };
    assert_ne!(
        canonical_batch_op(
            "",
            &chmod(ResourceRef::Store {
                store: ["d".into(), "s".into()]
            })
        )
        .unwrap(),
        canonical_batch_op(
            "",
            &chmod(ResourceRef::Table {
                table: ["d".into(), "s".into(), "".into()]
            })
        )
        .unwrap()
    );
    assert_ne!(
        canonical_batch_op(
            "",
            &chmod(ResourceRef::FunctionFolder {
                function_folder: vec!["a".into(), "b".into()]
            })
        )
        .unwrap(),
        canonical_batch_op(
            "",
            &chmod(ResourceRef::FunctionFolder {
                function_folder: vec!["a/b".into()]
            })
        )
        .unwrap()
    );
    let named_folder = BatchOp::Chmod(crate::admin::ChmodOp {
        chmod: ResourceRef::FunctionFolder {
            function_folder: vec!["a/b".into()],
        },
        mode: 0,
        hmac: None,
    });
    let split_folder = BatchOp::Chmod(crate::admin::ChmodOp {
        chmod: ResourceRef::FunctionFolder {
            function_folder: vec!["a".into(), "b".into()],
        },
        mode: 0,
        hmac: None,
    });
    assert_ne!(
        canonical_batch_op("", &named_folder).unwrap(),
        canonical_batch_op("", &split_folder).unwrap()
    );
}
#[test]
fn rv_query_types_f1dc8e9a_v2_prefix_and_tag_are_pinned() {
    let op = BatchOp::DropDb(DropDbOp {
        drop_db: "x".into(),
        hmac: None,
        cascade: false,
        if_exists: false,
    });
    let b = canonical_batch_op("", &op).unwrap();
    assert_eq!(&b[..35], b"shamir-db hmac canonical v2\0\0\x01\0\0\0\x01x");
    assert_eq!(&b[35..], b"\0\0");

    // v2 prefix + CreateFunction(0x0016): name, replace, security,
    // visibility, secret grants, net grants, source digest, wasm digest.
    let create_function = BatchOp::CreateFunction(CreateFunctionOp {
        create_function: "f".into(),
        source: None,
        wasm: None,
        replace: false,
        visibility: None,
        security: None,
        secret_grants: vec![],
        net_grants: vec![],
        hmac: None,
    });
    assert_eq!(
        canonical_batch_op("", &create_function).unwrap(),
        b"shamir-db hmac canonical v2\0\0\x16\0\0\0\x01f\0\0\0\0\x07invoker\0\0\0\0\0\0\0\0\0\0\0",
    );

    let set_superuser = DbRequest::SetSuperuser {
        user: "u".into(),
        on: true,
        hmac: None,
    };
    assert_eq!(
        crate::hmac::canonical_db_request(&set_superuser).unwrap(),
        b"shamir-db hmac canonical v2\0\0\x17\0\0\0\x01u\x01"
    );

    let set_replicator = DbRequest::SetReplicator {
        user: "u".into(),
        on: false,
        hmac: None,
    };
    assert_eq!(
        crate::hmac::canonical_db_request(&set_replicator).unwrap(),
        b"shamir-db hmac canonical v2\0\0\x18\0\0\0\x01u\0"
    );

    let create_scram_user = DbRequest::CreateScramUser {
        name: "u".into(),
        password: SecretString::from("ignored".to_owned()),
        roles: vec!["r".into()],
        hmac: None,
    };
    assert_eq!(
        crate::hmac::canonical_db_request(&create_scram_user).unwrap(),
        b"shamir-db hmac canonical v2\0\0\x19\0\0\0\x01u\0\0\0\x01\0\0\0\x01r"
    );
}
#[test]
fn canonical_required_drop_table_changes_on_every_flag() {
    let mk = |cascade, if_exists| {
        BatchOp::DropTable(DropTableOp {
            drop_table: "t".into(),
            repo: "r".into(),
            hmac: None,
            if_exists,
            cascade,
        })
    };
    let base = canonical_batch_op("d", &mk(false, false));
    assert_ne!(base, canonical_batch_op("d", &mk(true, false)));
    assert_ne!(base, canonical_batch_op("d", &mk(false, true)));
}
#[test]
fn rv_query_types_9cde03ef_required_schema_flag_list_option_flips() {
    let cases: Vec<(&str, Vec<BatchOp>)> = vec![
        (
            "drop_repo",
            vec![
                BatchOp::DropRepo(crate::admin::DropRepoOp {
                    drop_repo: "r".into(),
                    hmac: None,
                    cascade: false,
                    if_exists: false,
                }),
                BatchOp::DropRepo(crate::admin::DropRepoOp {
                    drop_repo: "r".into(),
                    hmac: None,
                    cascade: true,
                    if_exists: false,
                }),
                BatchOp::DropRepo(crate::admin::DropRepoOp {
                    drop_repo: "r".into(),
                    hmac: None,
                    cascade: false,
                    if_exists: true,
                }),
            ],
        ),
        (
            "drop_index",
            vec![
                BatchOp::DropIndex(crate::admin::DropIndexOp {
                    drop_index: "i".into(),
                    table: "t".into(),
                    repo: "r".into(),
                    hmac: None,
                    if_exists: false,
                    request_id: None,
                }),
                BatchOp::DropIndex(crate::admin::DropIndexOp {
                    drop_index: "i".into(),
                    table: "t".into(),
                    repo: "r".into(),
                    hmac: None,
                    if_exists: true,
                    request_id: None,
                }),
            ],
        ),
        (
            "drop_user",
            vec![
                BatchOp::DropUser(crate::auth::DropUserOp {
                    drop_user: "u".into(),
                    hmac: None,
                    if_exists: false,
                }),
                BatchOp::DropUser(crate::auth::DropUserOp {
                    drop_user: "u".into(),
                    hmac: None,
                    if_exists: true,
                }),
            ],
        ),
        (
            "drop_group",
            vec![
                BatchOp::DropGroup(crate::admin::DropGroupOp {
                    drop_group: GroupRef::Name { name: "g".into() },
                    if_exists: false,
                    hmac: None,
                }),
                BatchOp::DropGroup(crate::admin::DropGroupOp {
                    drop_group: GroupRef::Name { name: "g".into() },
                    if_exists: true,
                    hmac: None,
                }),
            ],
        ),
        (
            "drop_function",
            vec![
                BatchOp::DropFunction(crate::admin::DropFunctionOp {
                    drop_function: "f".into(),
                    if_exists: false,
                    hmac: None,
                }),
                BatchOp::DropFunction(crate::admin::DropFunctionOp {
                    drop_function: "f".into(),
                    if_exists: true,
                    hmac: None,
                }),
            ],
        ),
        (
            "drop_validator",
            vec![
                BatchOp::DropValidator(crate::admin::DropValidatorOp {
                    drop_validator: "v".into(),
                    if_exists: false,
                    hmac: None,
                }),
                BatchOp::DropValidator(crate::admin::DropValidatorOp {
                    drop_validator: "v".into(),
                    if_exists: true,
                    hmac: None,
                }),
            ],
        ),
        (
            "create_function_source_and_replace",
            vec![
                BatchOp::CreateFunction(CreateFunctionOp {
                    create_function: "f".into(),
                    source: None,
                    wasm: None,
                    replace: false,
                    visibility: None,
                    security: None,
                    secret_grants: vec![],
                    net_grants: vec![],
                    hmac: None,
                }),
                BatchOp::CreateFunction(CreateFunctionOp {
                    create_function: "f".into(),
                    source: Some("src".into()),
                    wasm: None,
                    replace: false,
                    visibility: None,
                    security: None,
                    secret_grants: vec![],
                    net_grants: vec![],
                    hmac: None,
                }),
                BatchOp::CreateFunction(CreateFunctionOp {
                    create_function: "f".into(),
                    source: None,
                    wasm: Some("wasm".into()),
                    replace: false,
                    visibility: None,
                    security: None,
                    secret_grants: vec![],
                    net_grants: vec![],
                    hmac: None,
                }),
            ],
        ),
        (
            "create_validator",
            vec![
                BatchOp::CreateValidator(crate::admin::CreateValidatorOp {
                    create_validator: "v".into(),
                    source: None,
                    wasm: None,
                    replace: false,
                    hmac: None,
                }),
                BatchOp::CreateValidator(crate::admin::CreateValidatorOp {
                    create_validator: "v".into(),
                    source: Some("src".into()),
                    wasm: None,
                    replace: false,
                    hmac: None,
                }),
                BatchOp::CreateValidator(crate::admin::CreateValidatorOp {
                    create_validator: "v".into(),
                    source: None,
                    wasm: Some("wasm".into()),
                    replace: false,
                    hmac: None,
                }),
                BatchOp::CreateValidator(crate::admin::CreateValidatorOp {
                    create_validator: "v".into(),
                    source: None,
                    wasm: None,
                    replace: true,
                    hmac: None,
                }),
            ],
        ),
        (
            "start_migration",
            vec![
                BatchOp::StartMigration(crate::admin::StartMigrationOp {
                    start_migration: "t".into(),
                    repo: "r".into(),
                    dst_repo: "r2".into(),
                    dst_engine: "e".into(),
                    dst_path: None,
                    hmac: None,
                }),
                BatchOp::StartMigration(crate::admin::StartMigrationOp {
                    start_migration: "t".into(),
                    repo: "r".into(),
                    dst_repo: "r2".into(),
                    dst_engine: "e".into(),
                    dst_path: Some("path".into()),
                    hmac: None,
                }),
            ],
        ),
        (
            "set_retention",
            vec![
                BatchOp::SetRetention(crate::admin::SetRetentionOp {
                    set_retention: "t".into(),
                    repo: "r".into(),
                    retention: Default::default(),
                    hmac: None,
                }),
                BatchOp::SetRetention(crate::admin::SetRetentionOp {
                    set_retention: "t".into(),
                    repo: "r".into(),
                    retention: crate::admin::Retention {
                        max_age_secs: Some(1),
                        max_count: None,
                        min_count: None,
                    },
                    hmac: None,
                }),
                BatchOp::SetRetention(crate::admin::SetRetentionOp {
                    set_retention: "t".into(),
                    repo: "r".into(),
                    retention: crate::admin::Retention {
                        max_age_secs: None,
                        max_count: Some(1),
                        min_count: None,
                    },
                    hmac: None,
                }),
                BatchOp::SetRetention(crate::admin::SetRetentionOp {
                    set_retention: "t".into(),
                    repo: "r".into(),
                    retention: crate::admin::Retention {
                        max_age_secs: None,
                        max_count: None,
                        min_count: Some(1),
                    },
                    hmac: None,
                }),
            ],
        ),
        (
            "create_user",
            vec![
                BatchOp::CreateUser(crate::auth::CreateUserOp {
                    create_user: "u".into(),
                    password: SecretString::from("pw".to_owned()),
                    roles: vec![],
                    database: None,
                    hmac: None,
                }),
                BatchOp::CreateUser(crate::auth::CreateUserOp {
                    create_user: "u".into(),
                    password: SecretString::from("pw".to_owned()),
                    roles: vec!["r".into()],
                    database: None,
                    hmac: None,
                }),
                BatchOp::CreateUser(crate::auth::CreateUserOp {
                    create_user: "u".into(),
                    password: SecretString::from("pw".to_owned()),
                    roles: vec![],
                    database: Some("d".into()),
                    hmac: None,
                }),
            ],
        ),
        (
            "drop_db",
            vec![
                BatchOp::DropDb(DropDbOp {
                    drop_db: "db".into(),
                    hmac: None,
                    cascade: false,
                    if_exists: false,
                }),
                BatchOp::DropDb(DropDbOp {
                    drop_db: "db".into(),
                    hmac: None,
                    cascade: true,
                    if_exists: false,
                }),
                BatchOp::DropDb(DropDbOp {
                    drop_db: "db".into(),
                    hmac: None,
                    cascade: false,
                    if_exists: true,
                }),
            ],
        ),
        (
            "drop_table",
            vec![
                BatchOp::DropTable(DropTableOp {
                    drop_table: "t".into(),
                    repo: "r".into(),
                    hmac: None,
                    if_exists: false,
                    cascade: false,
                }),
                BatchOp::DropTable(DropTableOp {
                    drop_table: "t".into(),
                    repo: "r".into(),
                    hmac: None,
                    if_exists: false,
                    cascade: true,
                }),
                BatchOp::DropTable(DropTableOp {
                    drop_table: "t".into(),
                    repo: "r".into(),
                    hmac: None,
                    if_exists: true,
                    cascade: false,
                }),
            ],
        ),
        (
            "create_function",
            vec![
                BatchOp::CreateFunction(CreateFunctionOp {
                    create_function: "f".into(),
                    source: None,
                    wasm: None,
                    replace: false,
                    visibility: None,
                    security: None,
                    secret_grants: vec![],
                    net_grants: vec![],
                    hmac: None,
                }),
                BatchOp::CreateFunction(CreateFunctionOp {
                    create_function: "f".into(),
                    source: None,
                    wasm: None,
                    replace: true,
                    visibility: None,
                    security: None,
                    secret_grants: vec![],
                    net_grants: vec![],
                    hmac: None,
                }),
                BatchOp::CreateFunction(CreateFunctionOp {
                    create_function: "f".into(),
                    source: None,
                    wasm: None,
                    replace: false,
                    visibility: Some("private".into()),
                    security: None,
                    secret_grants: vec![],
                    net_grants: vec![],
                    hmac: None,
                }),
                BatchOp::CreateFunction(CreateFunctionOp {
                    create_function: "f".into(),
                    source: None,
                    wasm: None,
                    replace: false,
                    visibility: None,
                    security: None,
                    secret_grants: vec!["a".into()],
                    net_grants: vec![],
                    hmac: None,
                }),
                BatchOp::CreateFunction(CreateFunctionOp {
                    create_function: "f".into(),
                    source: None,
                    wasm: None,
                    replace: false,
                    visibility: None,
                    security: None,
                    secret_grants: vec![],
                    net_grants: vec!["a".into()],
                    hmac: None,
                }),
            ],
        ),
    ];
    for (schema, variants) in cases {
        let base = canonical_batch_op("d", &variants[0]).unwrap();
        for variant in variants.iter().skip(1) {
            assert_ne!(base, canonical_batch_op("d", variant).unwrap(), "{schema}");
        }
    }
}

#[test]
fn rv_query_types_9cde03ef_remaining_optional_and_boolean_flips_are_bound() {
    let chgrp = |group| {
        BatchOp::Chgrp(crate::admin::ChgrpOp {
            chgrp: ResourceRef::Function {
                function: "f".into(),
            },
            group,
            hmac: None,
        })
    };
    assert_ne!(
        canonical_batch_op("", &chgrp(None)).unwrap(),
        canonical_batch_op("", &chgrp(Some(3))).unwrap()
    );

    let function = |wasm| {
        BatchOp::CreateFunction(CreateFunctionOp {
            create_function: "f".into(),
            source: None,
            wasm,
            replace: false,
            visibility: None,
            security: None,
            secret_grants: vec![],
            net_grants: vec![],
            hmac: None,
        })
    };
    assert_ne!(
        canonical_batch_op("", &function(None)).unwrap(),
        canonical_batch_op("", &function(Some("bytes".into()))).unwrap()
    );
    assert_ne!(
        canonical_batch_op("", &function(None)).unwrap(),
        canonical_batch_op(
            "",
            &BatchOp::CreateFunction(CreateFunctionOp {
                create_function: "f".into(),
                source: None,
                wasm: None,
                replace: false,
                visibility: None,
                security: Some("definer".into()),
                secret_grants: vec![],
                net_grants: vec![],
                hmac: None,
            })
        )
        .unwrap()
    );

    let superuser = |on| DbRequest::SetSuperuser {
        user: "u".into(),
        on,
        hmac: None,
    };
    assert_ne!(
        crate::hmac::canonical_db_request(&superuser(false)).unwrap(),
        crate::hmac::canonical_db_request(&superuser(true)).unwrap()
    );
    let replicator = |on| DbRequest::SetReplicator {
        user: "u".into(),
        on,
        hmac: None,
    };
    assert_ne!(
        crate::hmac::canonical_db_request(&replicator(false)).unwrap(),
        crate::hmac::canonical_db_request(&replicator(true)).unwrap()
    );

    let roles = |roles| DbRequest::CreateScramUser {
        name: "u".into(),
        password: SecretString::from("ignored".to_owned()),
        roles,
        hmac: None,
    };
    assert_ne!(
        crate::hmac::canonical_db_request(&roles(vec![])).unwrap(),
        crate::hmac::canonical_db_request(&roles(vec!["r".into()])).unwrap()
    );
    assert_ne!(
        crate::hmac::canonical_db_request(&roles(vec!["a".into(), "b".into()])).unwrap(),
        crate::hmac::canonical_db_request(&roles(vec!["b".into(), "a".into()])).unwrap()
    );
}

#[test]
fn policy_explicitly_requires_confirmation_for_drop_db() {
    let op = BatchOp::DropDb(DropDbOp {
        drop_db: "x".into(),
        hmac: None,
        cascade: false,
        if_exists: false,
    });
    assert_eq!(hmac_policy(&op), HmacPolicy::Required);
}
