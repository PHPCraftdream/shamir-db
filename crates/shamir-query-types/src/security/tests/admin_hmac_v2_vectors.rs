use std::collections::BTreeSet;

use serde::Deserialize;

use crate::admin::{
    AddGroupMemberOp, ChgrpOp, ChmodOp, ChownOp, CreateFunctionOp, CreateGroupOp,
    CreateValidatorOp, DropDbOp, DropGroupOp, DropIndexOp, DropRepoOp, DropTableOp, GroupRef,
    PurgeHistoryOp, PurgeScope, RemoveGroupMemberOp, RenameGroupOp, ResourceRef, Retention,
    SetRetentionOp,
};
use crate::auth::{CreateUserOp, DropUserOp, GrantRoleOp, RevokeRoleOp};
use crate::batch::BatchOp;
use crate::hmac::{
    canonical_batch_op, canonical_db_request, compute_tag_hex, derive_session_hmac_key,
    verify_tag_hex,
};
use crate::wire::DbRequest;

#[derive(Deserialize)]
struct VectorFixture {
    session_id_hex: String,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    op_tag: String,
    op: String,
    fields: serde_json::Value,
    fields_hex: String,
    canonical_hex: String,
    tag_hex: String,
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0, "hex has odd length");
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => panic!("invalid hex digit"),
            };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect()
}

fn string(f: &serde_json::Value, key: &str) -> String {
    f[key].as_str().expect("string field").to_owned()
}
fn opt_string(f: &serde_json::Value, key: &str) -> Option<String> {
    f[key].as_str().map(str::to_owned)
}
fn boolean(f: &serde_json::Value, key: &str) -> bool {
    f[key].as_u64().expect("boolean as 0/1") != 0
}
fn u64_field(f: &serde_json::Value, key: &str) -> u64 {
    match &f[key] {
        serde_json::Value::String(n) => n.parse().expect("decimal u64 string"),
        serde_json::Value::Number(n) => n.as_u64().expect("u64 number"),
        _ => panic!("expected u64 field {key}"),
    }
}
fn opt_u64(f: &serde_json::Value, key: &str) -> Option<u64> {
    f.get(key)
        .filter(|value| !value.is_null())
        .map(|_| u64_field(f, key))
}
fn strings(f: &serde_json::Value, key: &str) -> Vec<String> {
    f[key]
        .as_array()
        .expect("string list")
        .iter()
        .map(|v| v.as_str().expect("list string").to_owned())
        .collect()
}
fn resource(f: &serde_json::Value) -> ResourceRef {
    let values = f["resource"].as_array().expect("resource components");
    let at = |i: usize| values[i].as_str().expect("resource string").to_owned();
    match f["resource_kind"].as_str().expect("resource kind") {
        "database" => ResourceRef::Database { database: at(0) },
        "store" => ResourceRef::Store {
            store: [at(0), at(1)],
        },
        "table" => ResourceRef::Table {
            table: [at(0), at(1), at(2)],
        },
        "function" => ResourceRef::Function { function: at(0) },
        "function_folder" => ResourceRef::FunctionFolder {
            function_folder: values
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect(),
        },
        "function_namespace" => ResourceRef::FunctionNamespace {
            function_namespace: true,
        },
        other => panic!("unknown resource kind {other}"),
    }
}
fn group(f: &serde_json::Value) -> GroupRef {
    match f["group_kind"].as_str().expect("group kind") {
        "name" => GroupRef::Name {
            name: string(f, "group"),
        },
        "id" => GroupRef::Id {
            id: f["group"]
                .as_str()
                .expect("decimal group id")
                .parse()
                .expect("u64 group id"),
        },
        other => panic!("unknown group kind {other}"),
    }
}
fn batch_op(v: &Vector) -> BatchOp {
    let f = &v.fields;
    match v.op.as_str() {
        "drop_db" => BatchOp::DropDb(DropDbOp {
            drop_db: string(f, "name"),
            hmac: None,
            cascade: boolean(f, "cascade"),
            if_exists: boolean(f, "if_exists"),
        }),
        "drop_repo" => BatchOp::DropRepo(DropRepoOp {
            drop_repo: string(f, "name"),
            hmac: None,
            cascade: boolean(f, "cascade"),
            if_exists: boolean(f, "if_exists"),
        }),
        "drop_table" => BatchOp::DropTable(DropTableOp {
            drop_table: string(f, "name"),
            repo: string(f, "repo"),
            hmac: None,
            if_exists: boolean(f, "if_exists"),
            cascade: boolean(f, "cascade"),
        }),
        "drop_index" => BatchOp::DropIndex(DropIndexOp {
            drop_index: string(f, "name"),
            table: string(f, "table"),
            repo: string(f, "repo"),
            hmac: None,
            if_exists: boolean(f, "if_exists"),
            request_id: None,
        }),
        "drop_user" => BatchOp::DropUser(DropUserOp {
            drop_user: string(f, "name"),
            hmac: None,
            if_exists: boolean(f, "if_exists"),
        }),
        "start_migration" => BatchOp::StartMigration(crate::admin::StartMigrationOp {
            start_migration: string(f, "name"),
            repo: string(f, "repo"),
            dst_repo: string(f, "dst_repo"),
            dst_engine: string(f, "dst_engine"),
            dst_path: opt_string(f, "dst_path"),
            hmac: None,
        }),
        "commit_migration" => BatchOp::CommitMigration(crate::admin::CommitMigrationOp {
            commit_migration: string(f, "name"),
            hmac: None,
        }),
        "rollback_migration" => BatchOp::RollbackMigration(crate::admin::RollbackMigrationOp {
            rollback_migration: string(f, "name"),
            hmac: None,
        }),
        "grant_role" => BatchOp::GrantRole(GrantRoleOp {
            grant_role: string(f, "role"),
            user: string(f, "user"),
            hmac: None,
        }),
        "revoke_role" => BatchOp::RevokeRole(RevokeRoleOp {
            revoke_role: string(f, "role"),
            user: string(f, "user"),
            hmac: None,
        }),
        "chmod" => BatchOp::Chmod(ChmodOp {
            chmod: resource(f),
            mode: u16::try_from(f["mode"].as_u64().unwrap()).unwrap(),
            hmac: None,
        }),
        "chown" => BatchOp::Chown(ChownOp {
            chown: resource(f),
            owner: u64_field(f, "owner"),
            hmac: None,
        }),
        "chgrp" => BatchOp::Chgrp(ChgrpOp {
            chgrp: resource(f),
            group: f["group_id"].as_u64(),
            hmac: None,
        }),
        "create_user" => BatchOp::CreateUser(CreateUserOp {
            create_user: string(f, "name"),
            password: crate::auth::SecretString::from("excluded".to_owned()),
            roles: strings(f, "roles"),
            database: opt_string(f, "database"),
            hmac: None,
        }),
        "set_retention" => BatchOp::SetRetention(SetRetentionOp {
            set_retention: string(f, "name"),
            repo: string(f, "repo"),
            retention: Retention {
                max_age_secs: opt_u64(f, "max_age_secs"),
                max_count: opt_u64(f, "max_count"),
                min_count: opt_u64(f, "min_count"),
            },
            hmac: None,
        }),
        "purge_history" => BatchOp::PurgeHistory(PurgeHistoryOp {
            purge_history: string(f, "name"),
            repo: string(f, "repo"),
            scope: if f["scope"] == "older_than" {
                PurgeScope::OlderThan {
                    timestamp: u64_field(f, "timestamp"),
                }
            } else {
                PurgeScope::OlderThanAge {
                    age_secs: u64_field(f, "age_secs"),
                }
            },
            hmac: None,
        }),
        "create_group" => BatchOp::CreateGroup(CreateGroupOp {
            create_group: string(f, "name"),
            hmac: None,
        }),
        "drop_group" => BatchOp::DropGroup(DropGroupOp {
            drop_group: group(f),
            if_exists: boolean(f, "if_exists"),
            hmac: None,
        }),
        "rename_group" => BatchOp::RenameGroup(RenameGroupOp {
            rename_group: group(f),
            to: string(f, "to"),
            hmac: None,
        }),
        "add_group_member" => BatchOp::AddGroupMember(AddGroupMemberOp {
            add_group_member: group(f),
            user: u64_field(f, "user"),
            hmac: None,
        }),
        "remove_group_member" => BatchOp::RemoveGroupMember(RemoveGroupMemberOp {
            remove_group_member: group(f),
            user: u64_field(f, "user"),
            hmac: None,
        }),
        "create_function" => BatchOp::CreateFunction(CreateFunctionOp {
            create_function: string(f, "name"),
            source: opt_string(f, "source"),
            wasm: opt_string(f, "wasm"),
            replace: boolean(f, "replace"),
            visibility: opt_string(f, "visibility"),
            security: opt_string(f, "security"),
            secret_grants: strings(f, "secret_grants"),
            net_grants: strings(f, "net_grants"),
            hmac: None,
        }),
        "create_validator" => BatchOp::CreateValidator(CreateValidatorOp {
            create_validator: string(f, "name"),
            source: opt_string(f, "source"),
            wasm: opt_string(f, "wasm"),
            replace: boolean(f, "replace"),
            hmac: None,
        }),
        "drop_function" => BatchOp::DropFunction(crate::admin::DropFunctionOp {
            drop_function: string(f, "name"),
            if_exists: boolean(f, "if_exists"),
            hmac: None,
        }),
        "drop_validator" => BatchOp::DropValidator(crate::admin::DropValidatorOp {
            drop_validator: string(f, "name"),
            if_exists: boolean(f, "if_exists"),
            hmac: None,
        }),
        "drop_replication_profile" => {
            BatchOp::DropReplicationProfile(crate::admin::DropReplicationProfileOp {
                drop_replication_profile: string(f, "name"),
                hmac: None,
            })
        }
        "drop_publication" => BatchOp::DropPublication(crate::admin::DropPublicationOp {
            drop_publication: string(f, "name"),
            hmac: None,
        }),
        "drop_subscription" => BatchOp::DropSubscription(crate::admin::DropSubscriptionOp {
            drop_subscription: string(f, "name"),
            hmac: None,
        }),
        other => panic!("unknown vector op {other}"),
    }
}

#[test]
fn admin_hmac_v2_fixture_matches_all_op_tags_and_tags_f1dc8e9a() {
    let fixture: VectorFixture =
        serde_json::from_str(include_str!("../../../test-vectors/admin_hmac_v2.json"))
            .expect("parse shared admin HMAC vectors");
    let session: [u8; 32] = hex_bytes(&fixture.session_id_hex)
        .try_into()
        .expect("32-byte session id");
    let key = derive_session_hmac_key(&session);
    assert_eq!(fixture.vectors.len(), 31);
    let mut seen = BTreeSet::new();
    for v in &fixture.vectors {
        let tag: u16 = u16::from_str_radix(&v.op_tag, 16).expect("op tag hex");
        assert!((1..=31).contains(&tag), "unexpected tag {}", v.op_tag);
        assert!(seen.insert(tag), "duplicate op tag {}", v.op_tag);
        let db = opt_string(&v.fields, "db").unwrap_or_default();
        let canonical = match v.op.as_str() {
            "set_superuser" => canonical_db_request(&DbRequest::SetSuperuser {
                user: string(&v.fields, "user"),
                on: boolean(&v.fields, "on"),
                hmac: None,
            })
            .map_err(|error| format!("canonical encoding failed: {error:?}"))
            .expect("set_superuser canonical encoding"),
            "set_replicator" => canonical_db_request(&DbRequest::SetReplicator {
                user: string(&v.fields, "user"),
                on: boolean(&v.fields, "on"),
                hmac: None,
            })
            .map_err(|error| format!("canonical encoding failed: {error:?}"))
            .expect("set_replicator canonical encoding"),
            "create_scram_user" => canonical_db_request(&DbRequest::CreateScramUser {
                name: string(&v.fields, "name"),
                password: crate::auth::SecretString::from("excluded".to_owned()),
                roles: strings(&v.fields, "roles"),
                hmac: None,
            })
            .map_err(|error| format!("canonical encoding failed: {error:?}"))
            .expect("create_scram_user canonical encoding"),
            _ => canonical_batch_op(&db, &batch_op(v))
                .map_err(|error| format!("canonical encoding failed: {error:?}"))
                .expect("batch canonical encoding"),
        };
        let prefix_and_tag = format!(
            "7368616d69722d646220686d61632063616e6f6e6963616c20763200{:04x}",
            tag
        );
        let prefix_and_tag = hex_bytes(&prefix_and_tag);
        assert!(
            canonical.starts_with(&prefix_and_tag),
            "{} prefix/tag",
            v.op_tag
        );
        assert_eq!(
            canonical[prefix_and_tag.len()..],
            hex_bytes(&v.fields_hex),
            "{} fields",
            v.op_tag
        );
        assert_eq!(
            canonical,
            hex_bytes(&v.canonical_hex),
            "{} canonical",
            v.op_tag
        );
        let signed_tag = compute_tag_hex(&key, &canonical);
        assert_eq!(signed_tag, v.tag_hex, "{} HMAC", v.op_tag);
        assert!(
            verify_tag_hex(&key, &canonical, &v.tag_hex),
            "{} verifier",
            v.op_tag
        );
    }
    assert_eq!(seen, (1u16..=31).collect());
}
