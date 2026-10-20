//! Pairing of base/head inventories and the `conservation` findings.
//!
//! Passes over the two inventories:
//! 1. exact identity (package + kind + logical path + enclosing + impl
//!    head + name) — unchanged items, compared by fingerprint and test
//!    attributes;
//! 2. same (kind + name + enclosing + signature) — moves; accepted only
//!    when a `[[moved]]` entry is declared, otherwise the pair falls
//!    apart into ITEM_LOST + ITEM_ADDED;
//! 3. everything equal except the name, unique on both sides — renames,
//!    declared with `[[renamed]]`; the rename comparison normalises the
//!    item's OWN name out of the signature tokens (the signature keeps
//!    `fn helper` vs `fn helper2`, which would otherwise never pair).
//!
//! Remaining base items are losses, remaining head items additions.
//! Tests are never excusable: a lost test is `TEST_LOST` regardless of
//! the delta. Names-only counting is impossible by construction: an
//! identity pair with a different fingerprint is `BODY_CHANGED`, with a
//! different signature `SIGNATURE_CHANGED`, and a same-name/same-
//! enclosing leftover with a changed body never pairs — signature
//! equality plus fingerprint difference still pairs (pass 2) and
//! reports BODY_CHANGED.

use super::delta::Delta;
use super::item::{identity, weak_key, Item, TestKind};
use crate::common::finding::Report;
use std::collections::BTreeMap;

/// One accepted pairing and what drifted between the two sides.
struct Pair {
    b: usize,
    h: usize,
    body_changed: bool,
    /// Normalised signature tokens differ (a parameter/return type or
    /// bounds changed while the body may be identical).
    signature_changed: bool,
}

/// Runs the comparison and fills `report`. `delta` is consumed as
/// allowances are matched; leftovers become `UNUSED_ALLOWANCE`.
pub fn compare(base: &[Item], head: &[Item], delta: &mut Delta, report: &mut Report) {
    let mut base_left: Vec<usize> = (0..base.len()).collect();
    let mut head_left: Vec<usize> = (0..head.len()).collect();
    let mut pairs: Vec<Pair> = Vec::new();

    // Pass 1: exact identity.
    pair_pass(
        base,
        head,
        &mut base_left,
        &mut head_left,
        &mut pairs,
        identity,
    );

    // Pass 2: moves (kind + name + enclosing + equal signature). If more
    // than one head item carries the same key there is no way to choose
    // a pairing: UNPAIRABLE_DUPLICATE, everything left unpaired.
    for dup in pair_pass(
        base,
        head,
        &mut base_left,
        &mut head_left,
        &mut pairs,
        |it| format!("{}\u{1}{}", weak_key(it), it.signature),
    ) {
        report.add(
            "(matching)",
            1,
            "UNPAIRABLE_DUPLICATE",
            format!("two or more head items match one base item by kind/name/enclosing: {dup}"),
        );
    }

    // Pass 2 pairs are moves only when declared; otherwise unpair.
    let mut still: Vec<Pair> = Vec::new();
    for p in pairs.drain(..) {
        let (b, h) = (&base[p.b], &head[p.h]);
        if b.logical == h.logical && b.path == h.path {
            still.push(p); // unchanged item
        } else if consume_side(delta, "moved", b, h) {
            still.push(p);
        } else if b.logical == h.logical {
            // Whole-module file move: logical path kept, only the
            // physical file changed — a move by definition, still
            // declared so the delta tells the story.
            report.add(
                &h.path,
                h.line,
                "ITEM_MOVED",
                format!(
                    "`{}` moved files ({}:{}); declare [[moved]]",
                    h.logical, b.path, b.line
                ),
            );
            still.push(p);
        } else {
            base_left.push(p.b);
            head_left.push(p.h);
        }
    }
    pairs = still;

    // Pass 3: renames — equal kind/enclosing/fingerprint and equal
    // signature with the item's own name normalised out, different name,
    // unique head candidate.
    let mut rename_pairs: Vec<(usize, usize)> = Vec::new();
    base_left.retain(|&b| {
        let cands: Vec<usize> = head_left
            .iter()
            .copied()
            .filter(|&h| {
                let (b, h) = (&base[b], &head[h]);
                b.kind == h.kind
                    && b.package == h.package
                    && b.enclosing == h.enclosing
                    && b.impl_head == h.impl_head
                    && rename_sig(b) == rename_sig(h)
                    && b.fingerprint == h.fingerprint
                    && b.name != h.name
            })
            .collect();
        if cands.len() == 1 {
            rename_pairs.push((b, cands[0]));
            return false;
        }
        true
    });
    for (_b, h) in &rename_pairs {
        head_left.retain(|x| x != h);
    }
    for (bi, hi) in &rename_pairs {
        let (b, h) = (&base[*bi], &head[*hi]);
        if consume_side(delta, "renamed", b, h) {
            // Renamed pairs still report drift below via a synthetic pair.
            pairs.push(Pair {
                b: *bi,
                h: *hi,
                body_changed: b.fingerprint != h.fingerprint,
                signature_changed: rename_sig(b) != rename_sig(h),
            });
        } else {
            base_left.push(*bi);
            head_left.push(*hi);
        }
    }

    // Losses and additions. New test items are fine; lost tests never
    // pair with anything except exact/move/renamed pairing above, and
    // are always findings.
    let losses: Vec<usize> = base_left
        .iter()
        .copied()
        .filter(|&b| {
            if base[b].test.is_test() {
                return true;
            }
            !(delta.consume("removed", &base[b].path, &base[b].logical)
                || delta.consume("extracted", &base[b].path, &base[b].logical))
        })
        .collect();
    let additions: Vec<usize> = head_left
        .iter()
        .copied()
        .filter(|&h| {
            !head[h].test.is_test() && !delta.consume("added", &head[h].path, &head[h].logical)
        })
        .collect();

    for b in losses {
        let it = &base[b];
        if it.test.is_test() {
            report.add(
                &it.path,
                it.line,
                "TEST_LOST",
                format!(
                    "test `{}` is gone at head; removing tests is never declarable",
                    it.logical
                ),
            );
        } else {
            report.add(
                &it.path,
                it.line,
                "ITEM_LOST",
                format!(
                    "`{}` has no counterpart at head; declare [[removed]]",
                    it.logical
                ),
            );
        }
    }
    for h in additions {
        let it = &head[h];
        report.add(
            &it.path,
            it.line,
            "ITEM_ADDED",
            format!("`{}` is new at head; declare [[added]]", it.logical),
        );
    }

    // Drift on paired items: test attributes, body fingerprint and
    // signature. A single [[changed]] entry excuses both kinds of drift
    // on one item (consumed once).
    for p in &pairs {
        let (b, h) = (&base[p.b], &head[p.h]);
        report_test_change(b, h, report);
        let excused =
            (p.body_changed || p.signature_changed) && consume_side(delta, "changed", b, h);
        if p.signature_changed && !excused {
            report.add(
                &h.path,
                h.line,
                "SIGNATURE_CHANGED",
                format!(
                    "signature of `{}` differs from base ({}:{}): base `{}` vs head `{}`",
                    h.logical, b.path, b.line, b.signature, h.signature
                ),
            );
        }
        if p.body_changed && !excused {
            report.add(
                &h.path,
                h.line,
                "BODY_CHANGED",
                format!(
                    "body fingerprint of `{}` differs from base ({}:{})",
                    h.logical, b.path, b.line
                ),
            );
        }
    }

    // Unused allowances.
    for (kind, e) in delta_leftover(delta) {
        report.add(
            &e.path,
            1,
            "UNUSED_ALLOWANCE",
            format!(
                "[[{kind}]] entry `{}` matched nothing: {}",
                e.item, e.reason
            ),
        );
    }
}

/// Consumes an entry matching both sides of a pair (a move, rename,
/// extraction or changed-body allowance).
fn consume_side(delta: &mut Delta, kind: &str, b: &Item, h: &Item) -> bool {
    delta.consume_pair(kind, &b.path, &b.logical, &h.path, &h.logical)
        || delta.consume_pair(kind, &h.path, &h.logical, &b.path, &b.logical)
}

/// Greedy one-pass pairing on an exact key. Returns the ambiguous keys
/// (more than one head item carrying the same key as a base item).
fn pair_pass<F>(
    base: &[Item],
    head: &[Item],
    base_left: &mut Vec<usize>,
    head_left: &mut Vec<usize>,
    pairs: &mut Vec<Pair>,
    key: F,
) -> Vec<String>
where
    F: Fn(&Item) -> String,
{
    let mut by_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for &h in head_left.iter() {
        by_key.entry(key(&head[h])).or_default().push(h);
    }
    let mut ambiguous: Vec<String> = Vec::new();
    let mut still: Vec<usize> = Vec::new();
    for &b in base_left.iter() {
        let k = key(&base[b]);
        let hit = by_key.get_mut(&k);
        match hit {
            Some(v) if !v.is_empty() => {
                if v.len() > 1 {
                    // Cannot choose a pairing: leave base and heads open.
                    ambiguous.push(k);
                    still.push(b);
                    let dupes: Vec<usize> = std::mem::take(v);
                    for h in dupes {
                        still_head(head_left, h);
                    }
                } else {
                    let h = v.pop().expect("non-empty");
                    pairs.push(Pair {
                        b,
                        h,
                        body_changed: base[b].fingerprint != head[h].fingerprint,
                        signature_changed: base[b].signature != head[h].signature,
                    });
                    still_head(head_left, h);
                }
            }
            _ => still.push(b),
        }
    }
    *base_left = still;
    ambiguous
}

fn still_head(head_left: &mut Vec<usize>, h: usize) {
    head_left.retain(|x| *x != h);
}

fn delta_leftover(delta: &Delta) -> Vec<(&'static str, &super::delta::Entry)> {
    [
        ("moved", &delta.moved),
        ("renamed", &delta.renamed),
        ("extracted", &delta.extracted),
        ("removed", &delta.removed),
        ("added", &delta.added),
        ("changed", &delta.changed),
    ]
    .into_iter()
    .flat_map(|(k, l)| l.iter().map(move |e| (k, e)))
    .collect()
}

// ---------- test-drift reporting ----------

/// Test attributes and cfg must be equal on paired items; any drift is
/// reported per aspect.
fn report_test_change(b: &Item, h: &Item, report: &mut Report) {
    if !b.test.is_test() && !h.test.is_test() {
        return; // cfg drift of non-test items is the `structure` gate's job
    }
    let bt = &b.test;
    let ht = &h.test;
    if bt.kind != ht.kind {
        report.add(
            &h.path,
            h.line,
            "TEST_ATTR_CHANGED",
            format!(
                "test `{}` changed kind at head: {} -> {}",
                h.logical,
                kind_name(&bt.kind),
                kind_name(&ht.kind)
            ),
        );
    }
    if bt.ignored && !ht.ignored {
        report.add(
            &h.path,
            h.line,
            "TEST_UNIGNORED",
            format!("test `{}` is no longer #[ignore] at head", h.logical),
        );
    }
    if !bt.ignored && ht.ignored {
        let reason = ht
            .ignore_reason
            .clone()
            .unwrap_or_else(|| "no reason given".into());
        report.add(
            &h.path,
            h.line,
            "TEST_IGNORED",
            format!(
                "test `{}` is newly #[ignore]d at head ({reason})",
                h.logical
            ),
        );
    } else if bt.ignored && ht.ignored && bt.ignore_reason != ht.ignore_reason {
        report.add(
            &h.path,
            h.line,
            "TEST_ATTR_CHANGED",
            format!(
                "#[ignore] reason of `{}` changed: {:?} -> {:?}",
                h.logical, bt.ignore_reason, ht.ignore_reason
            ),
        );
    }
    if b.cfg != h.cfg {
        report.add(
            &h.path,
            h.line,
            "TEST_ATTR_CHANGED",
            format!(
                "cfg of test `{}` changed: {:?} -> {:?}",
                h.logical, b.cfg, h.cfg
            ),
        );
    }
}

fn kind_name(k: &TestKind) -> &'static str {
    match k {
        TestKind::None => "none",
        TestKind::Plain => "#[test]",
        TestKind::Tokio => "#[tokio::test]",
        TestKind::PropTest => "proptest!",
        TestKind::Loom => "loom model",
    }
}

/// Signature with the item's own name normalised out, for the RENAME
/// comparison only: extracted fn signatures keep the function name
/// (`fn helper (x : u32) -> u32` vs `fn helper2 (x : u32) -> u32`),
/// which would make a pure rename unpairable. Both sides reduce to
/// `fn (x : u32) -> u32`. Non-fn signatures are returned unchanged.
fn rename_sig(it: &Item) -> String {
    if it.kind == "fn" {
        if let Some(rest) = it.signature.strip_prefix("fn ") {
            if let Some(after) = rest.strip_prefix(it.name.as_str()) {
                return format!("fn{after}");
            }
        }
    }
    it.signature.clone()
}
