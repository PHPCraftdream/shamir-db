//! Tests for the `q!` proc-macro.

use shamir_query_types::read::ReadQuery;

use crate::filter as filter_mod;
use crate::q;
use crate::query::Query;
use crate::select;
use crate::write;

// ── helpers ────────────────────────────────────────────────────────

fn assert_same_wire(a: &ReadQuery, b: &ReadQuery) {
    let ja = serde_json::to_value(a).unwrap();
    let jb = serde_json::to_value(b).unwrap();
    assert_eq!(ja, jb, "wire JSON mismatch:\n  left:  {ja}\n  right: {jb}");
}

// ── from only ──────────────────────────────────────────────────────

#[test]
fn q_from_only() {
    let from_macro = q!(from users);
    let from_builder = Query::from("users").build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── from with string literal ───────────────────────────────────────

#[test]
fn q_from_string_literal() {
    let from_macro = q!(from "user_events");
    let from_builder = Query::from("user_events").build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── where ──────────────────────────────────────────────────────────

#[test]
fn q_where_simple() {
    let from_macro = q!(from users where age > 18);
    let from_builder = Query::from("users")
        .where_(filter_mod::gt("age", 18))
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── select ─────────────────────────────────────────────────────────

#[test]
fn q_select() {
    let from_macro = q!(from users select id, name, age);
    let from_builder = Query::from("users")
        .select([
            select::field("id"),
            select::field("name"),
            select::field("age"),
        ])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── where + select ─────────────────────────────────────────────────

#[test]
fn q_where_select() {
    let from_macro = q!(from users where status == "active" select id, name);
    let from_builder = Query::from("users")
        .where_(filter_mod::eq("status", "active"))
        .select([select::field("id"), select::field("name")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── group_by ───────────────────────────────────────────────────────

#[test]
fn q_group_by() {
    let from_macro = q!(from orders group_by city, country);
    let from_builder = Query::from("orders")
        .group_by_many(["city", "country"])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── order_by ───────────────────────────────────────────────────────

#[test]
fn q_order_by() {
    let from_macro = q!(from users order_by age desc, name asc);
    let from_builder = Query::from("users")
        .order_by_desc("age")
        .order_by_asc("name")
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── limit / offset ─────────────────────────────────────────────────

#[test]
fn q_limit() {
    let from_macro = q!(from users limit 20);
    let from_builder = Query::from("users").limit(20).build();
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn q_limit_offset() {
    let from_macro = q!(from users limit 20 offset 40);
    let from_builder = Query::from("users").limit(20).offset(40).build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── full spec example ──────────────────────────────────────────────

#[test]
fn q_full_spec() {
    let from_macro = q!(
        from users
        where age > 18
        select id, name
        order_by age desc
        limit 20
    );
    let from_builder = Query::from("users")
        .where_(filter_mod::gt("age", 18))
        .select([select::field("id"), select::field("name")])
        .order_by_desc("age")
        .limit(20)
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── compound where ─────────────────────────────────────────────────

#[test]
fn q_compound_where() {
    let from_macro = q!(
        from users
        where status == "active" && age > 18
        select id, name
    );
    let from_builder = Query::from("users")
        .where_(filter_mod::and([
            filter_mod::eq("status", "active"),
            filter_mod::gt("age", 18),
        ]))
        .select([select::field("id"), select::field("name")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── wire JSON snapshot ─────────────────────────────────────────────

#[test]
fn q_wire_json_snapshot() {
    let rq = q!(from users where age > 18 limit 10);
    let got = serde_json::to_value(&rq).unwrap();
    let expected = serde_json::json!({
        "from": "users",
        "select": {
            "items": [{"type": "all"}],
            "distinct": false
        },
        "where": {
            "op": "gt",
            "field": ["age"],
            "value": 18
        },
        "pagination": {
            "mode": "LimitOffset",
            "limit": 10,
            "offset": 0
        }
    });
    assert_eq!(got, expected);
}

// ── from repo.table ───────────────────────────────────────────────

#[test]
fn q_from_repo_table() {
    let from_macro = q!(from main.users);
    let from_builder = Query::with_repo("main", "users").build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── select with aliases ───────────────────────────────────────────

#[test]
fn q_select_field_as_alias() {
    let from_macro = q!(from users select id, name as user_name);
    let from_builder = Query::from("users")
        .select([select::field("id"), select::field_as("name", "user_name")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── select * ──────────────────────────────────────────────────────

#[test]
fn q_select_all() {
    let from_macro = q!(from users select *);
    let from_builder = Query::from("users").select([select::all()]).build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── count(*) ──────────────────────────────────────────────────────

#[test]
fn q_count_all_with_alias() {
    let from_macro = q!(from users select count(*) as n);
    let from_builder = Query::from("users")
        .select([select::count_all("n")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── built-in aggregates ───────────────────────────────────────────

#[test]
fn q_sum_aggregate() {
    let from_macro = q!(from orders select sum(amount) as total);
    let from_builder = Query::from("orders")
        .select([select::sum("amount", "total")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn q_avg_aggregate() {
    let from_macro = q!(from orders select avg(price) as avg_price);
    let from_builder = Query::from("orders")
        .select([select::avg("price", "avg_price")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn q_min_max_aggregate() {
    let from_macro = q!(from products select min(price) as cheapest, max(price) as priciest);
    let from_builder = Query::from("products")
        .select([
            select::min("price", "cheapest"),
            select::max("price", "priciest"),
        ])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn q_count_field_aggregate() {
    let from_macro = q!(from users select count(email) as email_count);
    let from_builder = Query::from("users")
        .select([select::count("email", "email_count")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── agg_fn ────────────────────────────────────────────────────────

#[test]
fn q_agg_fn_median() {
    let from_macro = q!(from users select agg_fn("median", age) as med);
    let from_builder = Query::from("users")
        .select([select::agg_fn("median", "age", "med")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── func in select ────────────────────────────────────────────────

#[test]
fn q_func_in_select() {
    use crate::val::col;
    let from_macro = q!(
        from users
        select func("strings/upper", [col("name")]) as up
    );
    let from_builder = Query::from("users")
        .select([select::func("up", "strings/upper", [col("name")])])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── select distinct ───────────────────────────────────────────────

#[test]
fn q_select_distinct() {
    let from_macro = q!(from users select distinct name, email);
    let from_builder = Query::from("users")
        .select([select::field("name"), select::field("email")])
        .distinct()
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── having ────────────────────────────────────────────────────────

#[test]
fn q_having_simple() {
    let from_macro = q!(
        from orders
        group_by city
        having total > 100
        select city
    );
    let from_builder = Query::from("orders")
        .group_by_many(["city"])
        .having(filter_mod::gt("total", 100))
        .select([select::field("city")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn q_having_complex_filter() {
    let from_macro = q!(
        from orders
        group_by city
        having total > 100 && qty >= 5
        select city, sum(total) as revenue
    );
    let from_builder = Query::from("orders")
        .group_by_many(["city"])
        .having(filter_mod::and([
            filter_mod::gt("total", 100),
            filter_mod::gte("qty", 5),
        ]))
        .select([select::field("city"), select::sum("total", "revenue")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── kitchen sink ──────────────────────────────────────────────────

#[test]
fn q_kitchen_sink() {
    let from_macro = q!(
        from main.orders
        where status == "paid" && total > 100
        group_by city
        having total > 50
        select city, sum(total) as revenue, count(*) as n
        order_by revenue desc
        limit 10
        offset 5
    );
    let from_builder = Query::with_repo("main", "orders")
        .where_(filter_mod::and([
            filter_mod::eq("status", "paid"),
            filter_mod::gt("total", 100),
        ]))
        .group_by_many(["city"])
        .having(filter_mod::gt("total", 50))
        .select([
            select::field("city"),
            select::sum("total", "revenue"),
            select::count_all("n"),
        ])
        .order_by_desc("revenue")
        .limit(10)
        .offset(5)
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── where with computed predicate ────────────────────────────────

#[test]
fn q_where_computed() {
    let from_macro = q!(from posts where computed("lower", email, "eq", "alice@foo.com"));
    let from_builder = Query::from("posts")
        .where_(filter_mod::computed(
            "lower",
            "email",
            "eq",
            "alice@foo.com",
        ))
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── clause order change: select now after having ──────────────────

#[test]
fn q_old_select_position_still_works() {
    // select right after where (before group_by) — should still work
    // as long as there's no group_by/having.
    let from_macro = q!(
        from users
        where age > 18
        select id, name
        order_by age desc
    );
    let from_builder = Query::from("users")
        .where_(filter_mod::gt("age", 18))
        .select([select::field("id"), select::field("name")])
        .order_by_desc("age")
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ── dotted select field ───────────────────────────────────────────

#[test]
fn q_select_dotted_field() {
    let from_macro = q!(from users select address.city as city);
    let from_builder = Query::from("users")
        .select([select::field_as(["address", "city"], "city")])
        .build();
    assert_same_wire(&from_macro, &from_builder);
}

// ══════════════════════════════════════════════════════════════════
// Write statement tests
// ══════════════════════════════════════════════════════════════════

// ── insert ────────────────────────────────────────────────────────

#[test]
fn q_insert_single_row() {
    let from_macro = q!(insert into users values {
        "name" => "Alice",
        "age" => 30
    });
    let from_builder = write::insert("users")
        .row(write::doc().set("name", "Alice").set("age", 30))
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(
        ja, jb,
        "insert single row mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}

#[test]
fn q_insert_multiple_rows() {
    let from_macro = q!(insert into users values {
        "name" => "Alice",
        "age" => 30
    }, {
        "name" => "Bob",
        "age" => 25
    });
    let from_builder = write::insert("users")
        .row(write::doc().set("name", "Alice").set("age", 30))
        .row(write::doc().set("name", "Bob").set("age", 25))
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(
        ja, jb,
        "insert multiple rows mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}

#[test]
fn q_insert_with_computed_field() {
    use crate::val::{col, func};

    let from_macro = q!(insert into users values {
        "email" => "Alice@X.COM",
        "email_norm" => func("strings/lower", [col("email")])
    });
    let from_builder = write::insert("users")
        .row(
            write::doc()
                .set("email", "Alice@X.COM")
                .set("email_norm", func("strings/lower", [col("email")])),
        )
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(
        ja, jb,
        "insert computed mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}

#[test]
fn q_insert_repo_qualified() {
    let from_macro = q!(insert into main.users values {
        "name" => "Alice"
    });
    let from_builder = write::Insert::with_repo("main", "users")
        .row(write::doc().set("name", "Alice"))
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(
        ja, jb,
        "insert repo qualified mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}

// ── update ────────────────────────────────────────────────────────

#[test]
fn q_update_with_complex_where() {
    let from_macro = q!(
        update users set { "tier" => "gold" }
        where total > 1000 && !is_null(email)
    );
    let from_builder = write::update("users")
        .set(write::doc().set("tier", "gold"))
        .where_(filter_mod::and([
            filter_mod::gt("total", 1000),
            filter_mod::not(filter_mod::is_null("email")),
        ]))
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(
        ja, jb,
        "update complex where mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}

#[test]
fn q_update_without_where() {
    let from_macro = q!(update users set { "active" => false });
    let from_builder = write::update("users")
        .set(write::doc().set("active", false))
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(
        ja, jb,
        "update without where mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}

// ── delete ────────────────────────────────────────────────────────

#[test]
fn q_delete_complex_where() {
    let from_macro = q!(
        delete from users
        where status == "deleted" && (in_(role, ["banned"]) || is_null(email)) && age < 18
    );
    let from_builder = write::delete("users")
        .where_(filter_mod::and([
            filter_mod::and([
                filter_mod::eq("status", "deleted"),
                filter_mod::or([
                    filter_mod::in_("role", ["banned"]),
                    filter_mod::is_null("email"),
                ]),
            ]),
            filter_mod::lt("age", 18),
        ]))
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(
        ja, jb,
        "delete complex where mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}

// ── upsert ────────────────────────────────────────────────────────

#[test]
fn q_upsert_key_value() {
    let from_macro = q!(
        upsert cache key { "id" => "k1" } value { "v" => 42 }
    );
    let from_builder = write::upsert("cache")
        .key(write::doc().set("id", "k1"))
        .value(write::doc().set("v", 42))
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(ja, jb, "upsert mismatch:\n  left:  {ja}\n  right: {jb}");
}

// ── batch composition ────────────────────────────────────────────

#[test]
fn q_write_composes_into_batch() {
    use crate::batch::Batch;

    let mut batch_macro = Batch::new();
    batch_macro.insert("a", q!(insert into users values { "name" => "Alice" }));

    let mut batch_builder = Batch::new();
    batch_builder.insert(
        "a",
        write::insert("users")
            .row(write::doc().set("name", "Alice"))
            .build(),
    );

    let ja = serde_json::to_value(batch_macro.build()).unwrap();
    let jb = serde_json::to_value(batch_builder.build()).unwrap();
    assert_eq!(
        ja, jb,
        "batch composition mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}

// ══════════════════════════════════════════════════════════════════
// Call statement tests
// ══════════════════════════════════════════════════════════════════

#[test]
fn q_call_ident_name_with_literals() {
    use shamir_query_types::call::CallOp;
    use shamir_query_types::filter::FilterValue;

    let op: CallOp = q!(call my_proc(1, "v"));
    assert_eq!(op.call, "my_proc");
    assert_eq!(op.repo, "main");
    assert_eq!(op.params.len(), 2);
    assert_eq!(op.params[0], FilterValue::Int(1));
    assert_eq!(op.params[1], FilterValue::String("v".to_string()));
}

#[test]
fn q_call_string_literal_name() {
    use shamir_query_types::call::CallOp;

    let op: CallOp = q!(call "reports/daily"(2024, "Q1"));
    assert_eq!(op.call, "reports/daily");
    assert_eq!(op.params.len(), 2);
}

#[test]
fn q_call_no_args() {
    use shamir_query_types::call::CallOp;

    let op: CallOp = q!(call ping());
    assert_eq!(op.call, "ping");
    assert!(op.params.is_empty());
}

#[test]
fn q_call_wire_json() {
    use shamir_query_types::call::CallOp;

    let op: CallOp = q!(call my_fn(42, "hello", true));
    let json = serde_json::to_value(&op).unwrap();
    let expected = serde_json::json!({
        "call": "my_fn",
        "params": [42, "hello", true],
        "repo": "main"
    });
    assert_eq!(json, expected);
}

#[test]
fn q_call_with_query_ref_expr() {
    use crate::val::qref;
    use shamir_query_types::call::CallOp;

    let op: CallOp = q!(call process(qref("users", "[0].id"), 100));
    assert_eq!(op.call, "process");
    assert_eq!(op.params.len(), 2);
    // First param is a $query ref
    let json = serde_json::to_value(&op.params[0]).unwrap();
    assert_eq!(json["$query"], "@users");
    assert_eq!(json["path"], "[0].id");
}

#[test]
fn q_call_composes_into_batch() {
    use crate::batch::Batch;

    let mut b = Batch::new();
    b.op("p", q!(call my_proc(1, "x")));
    let json = b.to_json_value().unwrap();
    assert_eq!(json["queries"]["p"]["call"], "my_proc");
}

// ── insert with trailing comma in doc ────────────────────────────

#[test]
fn q_insert_trailing_comma() {
    let from_macro = q!(insert into users values {
        "name" => "Alice",
        "age" => 30,
    });
    let from_builder = write::insert("users")
        .row(write::doc().set("name", "Alice").set("age", 30))
        .build();
    let ja = serde_json::to_value(&from_macro).unwrap();
    let jb = serde_json::to_value(&from_builder).unwrap();
    assert_eq!(
        ja, jb,
        "insert trailing comma mismatch:\n  left:  {ja}\n  right: {jb}"
    );
}
