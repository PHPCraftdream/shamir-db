//! Proc-macro crate for the ShamirDB guest function SDK.
//!
//! Exports the [`function`] attribute macro that transforms an author's async
//! function into a WebAssembly module exporting the ShamirDB guest ABI.

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, PatType, ReturnType};

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
