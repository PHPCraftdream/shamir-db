//! CallOp — stored procedure / callable function batch operation.
//!
//! A `CallOp` invokes a named WASM function as a top-level batch entry:
//! `{ "call": "fn_name", "params": [1, 2, "value"], "repo": "main" }`.
//!
//! The function runs server-side via `invoke_function_in_db_as` and returns
//! a `QueryResult` with the answer in `value` (object/array/scalar/null).

pub mod call_op;

#[cfg(test)]
mod tests;

pub use call_op::CallOp;
