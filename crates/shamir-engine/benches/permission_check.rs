//! Permission check per-request bench.
//!
//! `SessionPermissions::check(action, resource)` runs once per request
//! after auth. Walks pre-resolved decisions, picks most-specific match.
//! At the busiest hour every batch op needs at least one check; some
//! admin ops do several.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use shamir_engine::query::auth::SessionPermissions;
use shamir_query_types::auth::{Action, Effect, Permission, Resource, Role};

fn superadmin() -> Role {
    Role {
        name: "superadmin".into(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::All],
            resource: Resource::Global,
            row_filter: None,
        }],
    }
}

fn typical_role(db: &str, n_tables: usize) -> Role {
    let mut permissions = vec![
        Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read],
            resource: Resource::Database { database: db.into() },
            row_filter: None,
        },
        Permission {
            effect: Effect::Allow,
            actions: vec![Action::Insert, Action::Update, Action::Delete],
            resource: Resource::Repo {
                database: db.into(),
                repo: "main".into(),
            },
            row_filter: None,
        },
    ];
    for i in 0..n_tables {
        permissions.push(Permission {
            effect: Effect::Allow,
            actions: vec![Action::Alter],
            resource: Resource::Table {
                database: db.into(),
                repo: "main".into(),
                table: format!("table_{i}"),
            },
            row_filter: None,
        });
    }
    Role {
        name: "writer".into(),
        permissions,
    }
}

fn bench_check(c: &mut Criterion) {
    let mut group = c.benchmark_group("permission_check");
    group.throughput(Throughput::Elements(1));

    let target_table = Resource::Table {
        database: "prod".into(),
        repo: "main".into(),
        table: "users".into(),
    };
    let target_db = Resource::Database {
        database: "prod".into(),
    };

    // ─── superadmin fast path ─────────────────────────────────────
    let sp_super = SessionPermissions::build(&[superadmin()]);
    group.bench_function("superadmin_table", |b| {
        b.iter(|| black_box(sp_super.check(Action::Read, &target_table)))
    });

    // ─── typical role, 5 tables in role ──────────────────────────
    let sp_small = SessionPermissions::build(&[typical_role("prod", 5)]);
    group.bench_function("typical_5tables_table_hit", |b| {
        b.iter(|| black_box(sp_small.check(Action::Read, &target_table)))
    });
    group.bench_function("typical_5tables_db_hit", |b| {
        b.iter(|| black_box(sp_small.check(Action::Read, &target_db)))
    });

    // ─── role with 50 table-level decisions (RBAC scale stress) ──
    let sp_big = SessionPermissions::build(&[typical_role("prod", 50)]);
    group.bench_function("typical_50tables_table_hit", |b| {
        b.iter(|| black_box(sp_big.check(Action::Read, &target_table)))
    });

    // ─── deny path: action not in role ───────────────────────────
    group.bench_function("typical_5tables_deny", |b| {
        b.iter(|| black_box(sp_small.check(Action::ManageUsers, &target_table)))
    });

    group.finish();
}

criterion_group!(benches, bench_check);
criterion_main!(benches);
