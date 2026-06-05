//! Proc-macro crate for the ShamirDB guest function SDK.
//!
//! Exports the [`function`] attribute macro that transforms an author's async
//! function into a WebAssembly module exporting the ShamirDB guest ABI.

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, PatType, ReturnType, Type};

/// Attribute macro that turns an async validator function into a WASM guest
/// module.
///
/// **Only one `#[validator]` per crate is supported** (single entrypoint).
///
/// The author writes:
///
/// ```ignore
/// use shamir_sdk::prelude::*;
///
/// #[shamir_sdk::validator]
/// pub async fn check_user(record: Value, old_record: Option<Value>, ctx: Ctx) -> Validation {
///     if record == Value::Null {
///         return Validation::record_error("empty_record");
///     }
///     Validation::accept()
/// }
/// ```
///
/// The macro emits:
/// - The original function (renamed to `__shamir_impl_<name>`).
/// - `#[no_mangle] pub extern "C" fn shamir_alloc(len: i32) -> i32` — guest allocator.
/// - `#[no_mangle] pub extern "C" fn shamir_call(ptr: i32, len: i32) -> i64` — ABI entry.
///
/// The ABI entry:
/// 1. Decodes msgpack params into `Params`.
/// 2. Extracts `record = params.get("record")` as `Value`.
/// 3. Extracts `old_record = params.get("old_record")` → `Option<Value>` (absent or null → `None`).
/// 4. Builds a `Ctx`.
/// 5. Calls the author's function → `Validation`.
/// 6. Encodes `validation.into_value()` as msgpack and returns the packed `(ptr, len)`.
#[proc_macro_attribute]
pub fn validator(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let fn_item = parse_macro_input!(item as ItemFn);

    let fn_name = &fn_item.sig.ident;
    let inner_name = syn::Ident::new(&format!("__shamir_impl_{}", fn_name), fn_name.span());

    // Validate: must be async
    assert!(
        fn_item.sig.asyncness.is_some(),
        "#[validator] requires an async function"
    );

    // Validate: exactly 3 arguments (record, old_record, ctx)
    assert!(
        fn_item.sig.inputs.len() == 3,
        "#[validator] expects exactly 3 arguments: (record: Value, old_record: Option<Value>, ctx: Ctx)"
    );

    // Validate return type is Validation
    match &fn_item.sig.output {
        ReturnType::Type(_, ty) => {
            let type_str = quote!(#ty).to_string().replace(' ', "");
            assert!(
                type_str == "Validation",
                "#[validator] must return Validation, got: {type_str}"
            );
        }
        _ => panic!("#[validator] must return Validation"),
    }

    // Extract argument names
    let arg_names: Vec<_> = fn_item
        .sig
        .inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Typed(PatType { pat, .. }) => pat.clone(),
            _ => panic!("#[validator] expects named arguments"),
        })
        .collect();

    let arg0 = &arg_names[0]; // record
    let arg1 = &arg_names[1]; // old_record
    let arg2 = &arg_names[2]; // ctx

    let (impl_generics, _type_generics, where_clause) = fn_item.sig.generics.split_for_impl();

    let expanded = quote! {
        // The user's original function, renamed to a private inner.
        #[allow(non_snake_case)]
        async fn #inner_name #impl_generics(
            #arg0: shamir_sdk::Value,
            #arg1: Option<shamir_sdk::Value>,
            #arg2: shamir_sdk::Ctx,
        ) -> shamir_sdk::Validation
        #where_clause
        {
            #fn_item
            #fn_name(#arg0, #arg1, #arg2).await
        }

        /// Guest bump allocator: leak a `Vec<u8>` of `len` bytes, return its
        /// pointer. This memory is never freed — the WASM module is
        /// short-lived.
        #[no_mangle]
        pub extern "C" fn shamir_alloc(len: i32) -> i32 {
            let v: Vec<u8> = vec![0u8; len as usize];
            let ptr = v.as_ptr();
            core::mem::forget(v);
            ptr as i32
        }

        /// Guest ABI entry for a validator: decode params, extract record +
        /// old_record, drive the user's async function, encode the validation
        /// result, and return a packed `(ptr, len)`.
        #[no_mangle]
        pub extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
            // Safety: the host wrote `len` msgpack bytes at `ptr` via shamir_alloc.
            let bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(ptr as *const u8, len as usize)
            };

            let params = shamir_sdk::__rt::decode_params(bytes);

            // Extract `record` (required).
            let record: shamir_sdk::Value = match params.get("record") {
                Ok(v) => v.clone(),
                Err(_) => shamir_sdk::Value::Null,
            };

            // Extract `old_record` (optional: absent or null → None).
            let old_record: Option<shamir_sdk::Value> = match params.get("old_record") {
                Ok(shamir_sdk::Value::Null) => None,
                Ok(v) => Some(v.clone()),
                Err(_) => None,
            };

            let ctx = shamir_sdk::Ctx::new();

            let validation: shamir_sdk::Validation =
                shamir_sdk::__rt::block_on(#inner_name(record, old_record, ctx));

            let value = validation.into_value();
            let encoded = shamir_sdk::__rt::encode_value(&value);
            shamir_sdk::__rt::leak_result(encoded)
        }
    };

    TokenStream::from(expanded)
}

/// Attribute macro that turns an async function into a WASM guest module.
///
/// **Only one `#[function]` per crate is supported** (single entrypoint).
///
/// The author writes:
///
/// ```ignore
/// use shamir_sdk::prelude::*;
///
/// #[shamir_sdk::function]
/// pub async fn double(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
///     let n: i64 = params.i64("n")?;
///     Ok(Value::Int(n * 2))
/// }
/// ```
///
/// The macro emits:
/// - The original function (renamed to `__shamir_impl_<name>`).
/// - `#[no_mangle] pub extern "C" fn shamir_alloc(len: i32) -> i32` — guest allocator.
/// - `#[no_mangle] pub extern "C" fn shamir_call(ptr: i32, len: i32) -> i64` — ABI entry.
#[proc_macro_attribute]
pub fn function(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let fn_item = parse_macro_input!(item as ItemFn);

    let fn_name = &fn_item.sig.ident;
    let inner_name = syn::Ident::new(&format!("__shamir_impl_{}", fn_name), fn_name.span());

    // Validate signature: async fn name(ctx: Ctx, batch: Batch, params: Params) -> Result<Value>
    assert!(
        fn_item.sig.asyncness.is_some(),
        "#[function] requires an async function"
    );
    assert!(
        fn_item.sig.inputs.len() == 3,
        "#[function] expects exactly 3 arguments: (ctx: Ctx, batch: Batch, params: Params)"
    );

    // Validate return type is Result<Value>
    match &fn_item.sig.output {
        ReturnType::Type(_, ty) => {
            let type_str = quote!(#ty).to_string().replace(' ', "");
            assert!(
                type_str == "Result<Value>" || type_str == "core::result::Result<Value,Error>",
                "#[function] must return Result<Value>, got: {type_str}"
            );
        }
        _ => panic!("#[function] must return Result<Value>"),
    }

    // Extract argument names for the wrapper
    let arg_names: Vec<_> = fn_item
        .sig
        .inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Typed(PatType { pat, .. }) => pat.clone(),
            _ => panic!("#[function] expects named arguments"),
        })
        .collect();

    let arg0 = &arg_names[0]; // ctx
    let arg1 = &arg_names[1]; // batch
    let arg2 = &arg_names[2]; // params

    let (impl_generics, _type_generics, where_clause) = fn_item.sig.generics.split_for_impl();

    let expanded = quote! {
        // The user's original function, renamed to a private inner.
        #[allow(non_snake_case)]
        async fn #inner_name #impl_generics(
            #arg0: shamir_sdk::Ctx,
            #arg1: shamir_sdk::Batch,
            #arg2: shamir_sdk::Params,
        ) -> shamir_sdk::Result<shamir_sdk::Value>
        #where_clause
        {
            #fn_item
            #fn_name(#arg0, #arg1, #arg2).await
        }

        /// Guest bump allocator: leak a `Vec<u8>` of `len` bytes, return its
        /// pointer. This memory is never freed — the WASM module is
        /// short-lived.
        #[no_mangle]
        pub extern "C" fn shamir_alloc(len: i32) -> i32 {
            let v: Vec<u8> = vec![0u8; len as usize];
            let ptr = v.as_ptr();
            core::mem::forget(v);
            ptr as i32
        }

        /// Guest ABI entry: decode params, drive the user's async function,
        /// encode the result, and return a packed `(ptr, len)`.
        ///
        /// On user `Err(...)`: traps (the host maps a trap to
        /// `FunctionError::Compute`).
        // TODO(slice 4): clean Result envelope so user errors become FunctionError::User
        #[no_mangle]
        pub extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
            // Safety: the host wrote `len` msgpack bytes at `ptr` via shamir_alloc.
            let bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(ptr as *const u8, len as usize)
            };

            let params = shamir_sdk::__rt::decode_params(bytes);
            let ctx = shamir_sdk::Ctx::new();
            let batch = shamir_sdk::Batch::new();

            let result: shamir_sdk::Result<shamir_sdk::Value> =
                shamir_sdk::__rt::block_on(#inner_name(ctx, batch, params));

            match result {
                Ok(value) => {
                    let encoded = shamir_sdk::__rt::encode_value(&value);
                    shamir_sdk::__rt::leak_result(encoded)
                }
                Err(e) => {
                    shamir_sdk::__rt::trap(&e.to_string())
                }
            }
        }
    };

    TokenStream::from(expanded)
}

/// Attribute macro for a **procedure** — a guest function with database access.
///
/// **Only one `#[procedure]` per crate is supported** (single entrypoint).
///
/// The author writes:
///
/// ```ignore
/// use shamir_sdk::prelude::*;
///
/// #[shamir_sdk::procedure]
/// pub async fn my_proc(ctx: Ctx, params: Params) -> Result<Value> {
///     let rows = ctx.db().table("users").query(None);
///     Ok(rows)
/// }
/// ```
///
/// The macro emits the same guest ABI as `#[function]` (`shamir_alloc` +
/// `shamir_call`). The difference is the fixed 2-argument signature
/// `(ctx: Ctx, params: Params)` — `Batch` is not available (use
/// `#[function]` if you need the batch scratchpad).
///
/// Compile-time checks:
/// - Must be `async`.
/// - Exactly 2 arguments: `(ctx: Ctx, params: Params)`.
/// - Return type must be `Result<Value>`.
#[proc_macro_attribute]
pub fn procedure(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let fn_item = parse_macro_input!(item as ItemFn);

    let fn_name = &fn_item.sig.ident;
    let inner_name = syn::Ident::new(&format!("__shamir_impl_{}", fn_name), fn_name.span());

    // Validate: must be async
    assert!(
        fn_item.sig.asyncness.is_some(),
        "#[procedure] requires an async function"
    );

    // Validate: exactly 2 arguments (ctx, params)
    assert!(
        fn_item.sig.inputs.len() == 2,
        "#[procedure] expects exactly 2 arguments: (ctx: Ctx, params: Params)"
    );

    // Validate return type is Result<Value>
    match &fn_item.sig.output {
        ReturnType::Type(_, ty) => {
            let type_str = quote!(#ty).to_string().replace(' ', "");
            assert!(
                is_result_value_return(ty),
                "#[procedure] must return Result<Value>, got: {type_str}"
            );
        }
        _ => panic!("#[procedure] must return Result<Value>"),
    }

    // Extract argument names for the wrapper
    let arg_names: Vec<_> = fn_item
        .sig
        .inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Typed(PatType { pat, .. }) => pat.clone(),
            _ => panic!("#[procedure] expects named arguments"),
        })
        .collect();

    let arg0 = &arg_names[0]; // ctx
    let arg1 = &arg_names[1]; // params

    let (impl_generics, _type_generics, where_clause) = fn_item.sig.generics.split_for_impl();

    let expanded = quote! {
        // The user's original function, renamed to a private inner.
        #[allow(non_snake_case)]
        async fn #inner_name #impl_generics(
            #arg0: shamir_sdk::Ctx,
            #arg1: shamir_sdk::Params,
        ) -> shamir_sdk::Result<shamir_sdk::Value>
        #where_clause
        {
            #fn_item
            #fn_name(#arg0, #arg1).await
        }

        /// Guest bump allocator: leak a `Vec<u8>` of `len` bytes, return its
        /// pointer. This memory is never freed — the WASM module is
        /// short-lived.
        #[no_mangle]
        pub extern "C" fn shamir_alloc(len: i32) -> i32 {
            let v: Vec<u8> = vec![0u8; len as usize];
            let ptr = v.as_ptr();
            core::mem::forget(v);
            ptr as i32
        }

        /// Guest ABI entry for a procedure: decode params, build Ctx, drive
        /// the user's async function, encode the result, and return a packed
        /// `(ptr, len)`.
        #[no_mangle]
        pub extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
            // Safety: the host wrote `len` msgpack bytes at `ptr` via shamir_alloc.
            let bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(ptr as *const u8, len as usize)
            };

            let params = shamir_sdk::__rt::decode_params(bytes);
            let ctx = shamir_sdk::Ctx::new();

            let result: shamir_sdk::Result<shamir_sdk::Value> =
                shamir_sdk::__rt::block_on(#inner_name(ctx, params));

            match result {
                Ok(value) => {
                    let encoded = shamir_sdk::__rt::encode_value(&value);
                    shamir_sdk::__rt::leak_result(encoded)
                }
                Err(e) => {
                    shamir_sdk::__rt::trap(&e.to_string())
                }
            }
        }
    };

    TokenStream::from(expanded)
}

/// Returns `true` if the return type matches `Result<Value>` in any
/// qualification form (`Result<Value>`, `shamir_sdk::Result<shamir_sdk::Value>`,
/// `core::result::Result<Value, Error>`, etc.).
fn is_result_value_return(ty: &Type) -> bool {
    let s = quote!(#ty).to_string().replace(' ', "");
    // Strip any `shamir_sdk::` or `crate::` prefixes from the string so that
    // `shamir_sdk::Result<shamir_sdk::Value>` normalises to `Result<Value>`.
    let normalised = s
        .replace("shamir_sdk::", "")
        .replace("crate::", "")
        .replace("core::result::", "");
    normalised == "Result<Value>" || normalised == "Result<Value,Error>"
}

/// Returns `true` if the type token-stream contains `Ctx` anywhere.
///
/// Used by `#[scalar]` to reject `Ctx`-typed arguments and guarantee purity.
fn type_contains_ctx(ty: &Type) -> bool {
    let s = quote!(#ty).to_string();
    // Match standalone `Ctx` or fully-qualified `shamir_sdk::Ctx`, `crate::Ctx`
    // but not substrings like `CtxFoo`. We check word boundary via simple rules:
    // split on non-alphanumeric and check for exact "Ctx" segment.
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|segment| segment == "Ctx")
}

/// Attribute macro for a **scalar** — a pure value function with no database
/// access.
///
/// **Only one `#[scalar]` per crate is supported** (single entrypoint).
///
/// The author writes:
///
/// ```ignore
/// use shamir_sdk::prelude::*;
///
/// #[shamir_sdk::scalar]
/// pub async fn double(params: Params) -> Result<Value> {
///     let n: i64 = params.i64("n")?;
///     Ok(Value::Int(n * 2))
/// }
/// ```
///
/// The macro emits the same guest ABI (`shamir_alloc` + `shamir_call`).
/// The difference is the fixed 1-argument signature `(params: Params)` —
/// there is **no `Ctx`** available. This guarantees purity: a scalar
/// cannot access the database, call other functions, or perform HTTP
/// requests.
///
/// Compile-time checks:
/// - Must be `async`.
/// - Exactly 1 argument: `(params: Params)`.
/// - Return type must be `Result<Value>`.
/// - **No argument type may contain `Ctx`** — compile error if it does.
///   Use `#[procedure]` for functions that need database access.
#[proc_macro_attribute]
pub fn scalar(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let fn_item = parse_macro_input!(item as ItemFn);

    let fn_name = &fn_item.sig.ident;
    let inner_name = syn::Ident::new(&format!("__shamir_impl_{}", fn_name), fn_name.span());

    // Validate: must be async
    assert!(
        fn_item.sig.asyncness.is_some(),
        "#[scalar] requires an async function"
    );

    // Validate: exactly 1 argument (params)
    assert!(
        fn_item.sig.inputs.len() == 1,
        "#[scalar] expects exactly 1 argument: (params: Params). \
         Scalars are pure value functions — use #[procedure] if you need Ctx for database access."
    );

    // Reject Ctx in any argument type (purity guarantee)
    for arg in &fn_item.sig.inputs {
        if let FnArg::Typed(PatType { ty, .. }) = arg {
            assert!(
                !type_contains_ctx(ty),
                "#[scalar] must not take Ctx — scalars are pure value functions \
                 with no database access. Use #[procedure] for functions that need Ctx."
            );
        }
    }

    // Validate return type is Result<Value>
    match &fn_item.sig.output {
        ReturnType::Type(_, ty) => {
            let type_str = quote!(#ty).to_string().replace(' ', "");
            assert!(
                is_result_value_return(ty),
                "#[scalar] must return Result<Value>, got: {type_str}"
            );
        }
        _ => panic!("#[scalar] must return Result<Value>"),
    }

    // Extract argument names for the wrapper
    let arg_names: Vec<_> = fn_item
        .sig
        .inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Typed(PatType { pat, .. }) => pat.clone(),
            _ => panic!("#[scalar] expects named arguments"),
        })
        .collect();

    let arg0 = &arg_names[0]; // params

    let (impl_generics, _type_generics, where_clause) = fn_item.sig.generics.split_for_impl();

    let expanded = quote! {
        // The user's original function, renamed to a private inner.
        #[allow(non_snake_case)]
        async fn #inner_name #impl_generics(
            #arg0: shamir_sdk::Params,
        ) -> shamir_sdk::Result<shamir_sdk::Value>
        #where_clause
        {
            #fn_item
            #fn_name(#arg0).await
        }

        /// Guest bump allocator: leak a `Vec<u8>` of `len` bytes, return its
        /// pointer. This memory is never freed — the WASM module is
        /// short-lived.
        #[no_mangle]
        pub extern "C" fn shamir_alloc(len: i32) -> i32 {
            let v: Vec<u8> = vec![0u8; len as usize];
            let ptr = v.as_ptr();
            core::mem::forget(v);
            ptr as i32
        }

        /// Guest ABI entry for a scalar: decode params, drive the user's pure
        /// async function, encode the result, and return a packed `(ptr, len)`.
        #[no_mangle]
        pub extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
            // Safety: the host wrote `len` msgpack bytes at `ptr` via shamir_alloc.
            let bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(ptr as *const u8, len as usize)
            };

            let params = shamir_sdk::__rt::decode_params(bytes);

            let result: shamir_sdk::Result<shamir_sdk::Value> =
                shamir_sdk::__rt::block_on(#inner_name(params));

            match result {
                Ok(value) => {
                    let encoded = shamir_sdk::__rt::encode_value(&value);
                    shamir_sdk::__rt::leak_result(encoded)
                }
                Err(e) => {
                    shamir_sdk::__rt::trap(&e.to_string())
                }
            }
        }
    };

    TokenStream::from(expanded)
}
