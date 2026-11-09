//! Generated guests of a controlled binary size.
//!
//! Every module honours the procedure ABI (`memory`, `shamir_alloc`, `shamir_call`) and
//! carries unexported helper functions of about 290 bytes each (80 operators), so the
//! byte size grows with the function count while the export count stays at three and the
//! operator count per byte stays low enough for the default admission limits
//! (20,000 functions, 500,000 operators). The helpers are never called: they exist to be
//! compiled, which is what the size cells measure.

/// Target sizes: label and byte count.
pub const SIZES: [(&str, usize); 4] = [
    ("1k", 1 << 10),
    ("10k", 10 << 10),
    ("100k", 100 << 10),
    ("1m", 1 << 20),
];

/// `i32.mul`/`i32.add` pairs per helper (4 operators each).
const REPS: usize = 20;

fn helper(index: usize) -> String {
    let mut body = String::from("local.get 0\n");
    for rep in 0..REPS {
        // Constants stay in [300M, 2^31): five LEB bytes each, so the size is predictable.
        let mul = 300_000_000 + (index * 31 + rep * 7) % 1_000_000_000;
        let add = 400_000_000 + (index * 17 + rep * 13) % 1_000_000_000;
        body.push_str(&format!(
            "i32.const {mul}\ni32.mul\ni32.const {add}\ni32.add\n"
        ));
    }
    format!("(func (param i32) (result i32)\n{body})\n")
}

/// A binary module with `functions` helper functions.
pub fn binary(functions: usize) -> Vec<u8> {
    let helpers: String = (0..functions).map(helper).collect();
    let text = format!(
        r#"(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 4096))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  {helpers}
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.extend_i32_u (local.get $len)))))"#
    );
    wat::parse_str(text).expect("generated module is valid")
}

/// A module whose binary size is the closest below `target` bytes.
pub fn sized(target: usize) -> Vec<u8> {
    let small = binary(8);
    let large = binary(16);
    let per_function = (large.len() - small.len()) / 8;
    let base = small.len() - 8 * per_function;
    binary(target.saturating_sub(base) / per_function)
}
