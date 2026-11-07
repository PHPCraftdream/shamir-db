//! WAT guests for the `wasm_procedures` bench and their native twins.
//!
//! Every guest honours the procedure ABI (`memory`, `shamir_alloc`, `shamir_call`).
//! Compute kernels return `acc & 127` as a one-byte msgpack positive fixint, so the
//! host decodes it to `Int(acc & 127)` and the bench can check it against the native
//! twin: a kernel that silently computes something else would make the ratio to native
//! meaningless.

use std::hint::black_box;

/// Address the guests write their one-byte result to.
const RESULT_ADDR: u32 = 2048;

/// Packs `(RESULT_ADDR, 1)` as the `shamir_call` return value.
const RETURN_RESULT_BYTE: &str = "(i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 1))";

/// Stores `acc & 127` at [`RESULT_ADDR`] and returns it.
fn finish_with_acc() -> String {
    format!(
        "(i32.store8 (i32.const {RESULT_ADDR}) (i32.and (local.get $acc) (i32.const 127)))\n    {RETURN_RESULT_BYTE}"
    )
}

/// Wraps imports, helper items, locals and the `shamir_call` body into a procedure module.
fn module(imports: &str, helpers: &str, locals: &str, body: &str, pages: u32) -> String {
    format!(
        r#"(module
  {imports}
  (memory (export "memory") {pages})
  (global $bump (mut i32) (i32.const 4096))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  {helpers}
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    {locals}
    {body}))"#
    )
}

/// Echoes the parameter region back: the cheapest possible procedure.
pub fn identity() -> String {
    module(
        "",
        "",
        "",
        "(i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.extend_i32_u (local.get $len)))",
        2,
    )
}

/// A module with `n` exported helper functions besides the two ABI ones (compile-size probe).
pub fn many_functions(n: usize) -> String {
    assert!(n >= 2, "function count includes the two ABI exports");
    let helpers: String = (0..n - 2)
        .map(|i| {
            format!(
                "(func (export \"f{i}\") (param i32) (result i32) (i32.add (local.get 0) (i32.const {i})))\n  "
            )
        })
        .collect();
    module(
        "",
        &helpers,
        "",
        "(i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.extend_i32_u (local.get $len)))",
        2,
    )
}

// ── Compute kernels ──────────────────────────────────────────────────

const INT_SUM_N: u32 = 100_000;

pub fn int_sum_wat() -> String {
    module(
        "",
        "",
        "(local $i i32) (local $acc i32)",
        &format!(
            r#"(local.set $acc (i32.const 1))
    (block $done (loop $l
      (br_if $done (i32.ge_u (local.get $i) (i32.const {INT_SUM_N})))
      (local.set $acc (i32.add
        (i32.mul (local.get $acc) (i32.const 31))
        (i32.xor (local.get $i) (i32.shr_u (local.get $acc) (i32.const 3)))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $l)))
    {}"#,
            finish_with_acc()
        ),
        2,
    )
}

pub fn int_sum_native() -> u32 {
    let mut acc: u32 = 1;
    for i in 0..black_box(INT_SUM_N) {
        acc = acc.wrapping_mul(31).wrapping_add(i ^ (acc >> 3));
    }
    black_box(acc)
}

const F64_POLY_N: u32 = 50_000;

pub fn f64_poly_wat() -> String {
    module(
        "",
        "",
        "(local $i i32) (local $x f64) (local $acc i32)",
        &format!(
            r#"(local.set $x (f64.const 1.0))
    (block $done (loop $l
      (br_if $done (i32.ge_u (local.get $i) (i32.const {F64_POLY_N})))
      (local.set $x (f64.sub
        (f64.add (f64.mul (local.get $x) (f64.const 1.0000001)) (f64.const 0.5))
        (f64.mul (f64.div (local.get $x) (f64.add (local.get $x) (f64.const 3.0))) (f64.const 0.25))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $l)))
    (local.set $acc (i32.wrap_i64 (i64.trunc_sat_f64_s (local.get $x))))
    {}"#,
            finish_with_acc()
        ),
        2,
    )
}

pub fn f64_poly_native() -> u32 {
    let mut x: f64 = 1.0;
    for _ in 0..black_box(F64_POLY_N) {
        x = x * 1.0000001 + 0.5 - (x / (x + 3.0)) * 0.25;
    }
    black_box((x as i64) as i32 as u32)
}

const MEM_WORDS: u32 = 16_384;
const MEM_BASE: u32 = 65_536;
const MEM_PASSES: u32 = 4;

pub fn mem_scan_wat() -> String {
    module(
        "",
        "",
        "(local $i i32) (local $pass i32) (local $acc i32)",
        &format!(
            r#"(block $d1 (loop $l1
      (br_if $d1 (i32.ge_u (local.get $i) (i32.const {MEM_WORDS})))
      (i32.store
        (i32.add (i32.const {MEM_BASE}) (i32.shl (local.get $i) (i32.const 2)))
        (i32.mul (local.get $i) (i32.const 2654435761)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $l1)))
    (block $dp (loop $lp
      (br_if $dp (i32.ge_u (local.get $pass) (i32.const {MEM_PASSES})))
      (local.set $i (i32.const 0))
      (block $d2 (loop $l2
        (br_if $d2 (i32.ge_u (local.get $i) (i32.const {MEM_WORDS})))
        (local.set $acc (i32.add
          (i32.rotl (local.get $acc) (i32.const 5))
          (i32.load (i32.add (i32.const {MEM_BASE}) (i32.shl (local.get $i) (i32.const 2))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l2)))
      (local.set $pass (i32.add (local.get $pass) (i32.const 1)))
      (br $lp)))
    {}"#,
            finish_with_acc()
        ),
        2,
    )
}

pub fn mem_scan_native() -> u32 {
    let mut words = vec![0u32; MEM_WORDS as usize];
    for (i, word) in words.iter_mut().enumerate() {
        *word = (i as u32).wrapping_mul(2_654_435_761);
    }
    let words = black_box(words);
    let mut acc: u32 = 0;
    for _ in 0..MEM_PASSES {
        for word in &words {
            acc = acc.rotate_left(5).wrapping_add(*word);
        }
    }
    black_box(acc)
}

const COLLATZ_LIMIT: u32 = 1_500;

pub fn collatz_wat() -> String {
    module(
        "",
        "",
        "(local $n i32) (local $x i32) (local $acc i32)",
        &format!(
            r#"(local.set $n (i32.const 1))
    (block $d1 (loop $l1
      (br_if $d1 (i32.gt_u (local.get $n) (i32.const {COLLATZ_LIMIT})))
      (local.set $x (local.get $n))
      (block $d2 (loop $l2
        (br_if $d2 (i32.eq (local.get $x) (i32.const 1)))
        (if (i32.and (local.get $x) (i32.const 1))
          (then (local.set $x (i32.add (i32.mul (local.get $x) (i32.const 3)) (i32.const 1))))
          (else (local.set $x (i32.shr_u (local.get $x) (i32.const 1)))))
        (local.set $acc (i32.add (local.get $acc) (i32.const 1)))
        (br $l2)))
      (local.set $n (i32.add (local.get $n) (i32.const 1)))
      (br $l1)))
    {}"#,
            finish_with_acc()
        ),
        2,
    )
}

pub fn collatz_native() -> u32 {
    let mut acc: u32 = 0;
    for start in 1..=black_box(COLLATZ_LIMIT) {
        let mut x = start;
        while x != 1 {
            x = if x & 1 == 1 {
                x.wrapping_mul(3).wrapping_add(1)
            } else {
                x >> 1
            };
            acc = acc.wrapping_add(1);
        }
    }
    black_box(acc)
}

const FIB_N: u32 = 24;

pub fn fib_wat() -> String {
    module(
        "",
        r#"(func $fib (param $n i32) (result i32)
    (if (result i32) (i32.lt_u (local.get $n) (i32.const 2))
      (then (local.get $n))
      (else (i32.add
        (call $fib (i32.sub (local.get $n) (i32.const 1)))
        (call $fib (i32.sub (local.get $n) (i32.const 2)))))))"#,
        "(local $acc i32)",
        &format!(
            "(local.set $acc (call $fib (i32.const {FIB_N})))\n    {}",
            finish_with_acc()
        ),
        2,
    )
}

#[inline(never)]
fn fib(n: u32) -> u32 {
    if n < 2 {
        n
    } else {
        fib(n - 1).wrapping_add(fib(n - 2))
    }
}

pub fn fib_native() -> u32 {
    black_box(fib(black_box(FIB_N)))
}

/// One compute kernel: its guest, its native twin and the id used in the cell names.
pub struct Kernel {
    pub name: &'static str,
    pub wat: fn() -> String,
    pub native: fn() -> u32,
}

pub const KERNELS: [Kernel; 5] = [
    Kernel {
        name: "int_sum",
        wat: int_sum_wat,
        native: int_sum_native,
    },
    Kernel {
        name: "f64_poly",
        wat: f64_poly_wat,
        native: f64_poly_native,
    },
    Kernel {
        name: "mem_scan",
        wat: mem_scan_wat,
        native: mem_scan_native,
    },
    Kernel {
        name: "collatz",
        wat: collatz_wat,
        native: collatz_native,
    },
    Kernel {
        name: "fib",
        wat: fib_wat,
        native: fib_native,
    },
];

// ── Host-boundary guests ─────────────────────────────────────────────

/// Host calls per guest invocation in the boundary cells.
pub const HOST_CALLS: u32 = 100;

/// Calls the sync `batch_put` import [`HOST_CALLS`] times, then returns `Int(7)`.
pub fn sync_put_wat() -> String {
    module(
        r#"(import "shamir_host" "batch_put" (func $put (param i32 i32 i32 i32)))"#,
        r#"(data (i32.const 1024) "k")
  (data (i32.const 1032) "\07")"#,
        "(local $i i32)",
        &format!(
            r#"(block $done (loop $l
      (br_if $done (i32.ge_u (local.get $i) (i32.const {HOST_CALLS})))
      (call $put (i32.const 1024) (i32.const 1) (i32.const 1032) (i32.const 1))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $l)))
    (i32.store8 (i32.const {RESULT_ADDR}) (i32.const 7))
    {RETURN_RESULT_BYTE}"#
        ),
        2,
    )
}

/// Calls the async `batch_get` import [`HOST_CALLS`] times and returns the last answer
/// (the host wrote `Int(7)` into guest memory for it).
pub fn async_get_wat() -> String {
    module(
        r#"(import "shamir_host" "batch_get" (func $get (param i32 i32) (result i64)))"#,
        r#"(data (i32.const 1024) "k")"#,
        "(local $i i32) (local $last i64)",
        &format!(
            r#"(block $done (loop $l
      (br_if $done (i32.ge_u (local.get $i) (i32.const {HOST_CALLS})))
      (local.set $last (call $get (i32.const 1024) (i32.const 1)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $l)))
    (local.get $last)"#
        ),
        2,
    )
}
