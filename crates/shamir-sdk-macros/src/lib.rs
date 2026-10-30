//! Proc-macro crate for the ShamirDB guest function SDK.
//!
//! Exports the [`function`] attribute macro that transforms an author's async
//! function into a WebAssembly module exporting the ShamirDB guest ABI.
//!
//! Each attribute accepts `crate = path` to name a renamed SDK dependency;
//! without it the generated code uses `::shamir_sdk`.

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, PatType, PathArguments, ReturnType, Type};

struct SdkPath(syn::Path);

impl syn::parse::Parse for SdkPath {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(Self(syn::parse_quote!(::shamir_sdk)));
        }
        input.parse::<syn::Token![crate]>()?;
        input.parse::<syn::Token![=]>()?;
        let path = input.call(syn::Path::parse_mod_style)?;
        if input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
        }
        if !input.is_empty() {
            return Err(input.error("unexpected or duplicate SDK attribute option"));
        }
        Ok(Self(path))
    }
}

fn compile_error(err: syn::Error) -> TokenStream {
    err.to_compile_error().into()
}

fn validate_signature(
    fn_item: &ItemFn,
    name: &str,
    arity: usize,
    return_valid: bool,
) -> syn::Result<()> {
    let sig = &fn_item.sig;
    if sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            sig.fn_token,
            format!("#[{name}] requires an async function"),
        ));
    }
    if sig.inputs.len() != arity {
        return Err(syn::Error::new_spanned(
            &sig.inputs,
            format!("#[{name}] expects exactly {arity} arguments"),
        ));
    }
    if !return_valid {
        return Err(syn::Error::new_spanned(
            &sig.output,
            format!("#[{name}] has an invalid return type"),
        ));
    }
    for arg in &sig.inputs {
        if let FnArg::Receiver(receiver) = arg {
            return Err(syn::Error::new_spanned(
                receiver,
                format!("#[{name}] does not accept a `self` receiver"),
            ));
        }
    }
    Ok(())
}

fn helper_ident(fn_name: &syn::Ident) -> syn::Ident {
    let name = fn_name.to_string();
    let normalized = name.strip_prefix("r#").unwrap_or(&name);
    syn::Ident::new(&format!("__shamir_impl_{normalized}"), fn_name.span())
}

fn wrapper_idents(fn_item: &ItemFn) -> Vec<syn::Ident> {
    fn_item
        .sig
        .inputs
        .iter()
        .enumerate()
        .map(|(index, _)| {
            syn::Ident::new(
                &format!("__shamir_arg_{index}"),
                proc_macro2::Span::mixed_site(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests;
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
pub fn validator(attr: TokenStream, item: TokenStream) -> TokenStream {
    let SdkPath(sdk) = parse_macro_input!(attr as SdkPath);
    let fn_item = parse_macro_input!(item as ItemFn);
    match expand_validator(fn_item, &sdk) {
        Ok(expanded) => expanded.into(),
        Err(err) => compile_error(err),
    }
}

fn expand_validator(fn_item: ItemFn, sdk: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    let fn_name = &fn_item.sig.ident;
    let inner_name = helper_ident(fn_name);
    validate_signature(
        &fn_item,
        "validator",
        3,
        matches!(&fn_item.sig.output, ReturnType::Type(_, ty) if is_validation_return(ty)),
    )?;
    let arg_names = wrapper_idents(&fn_item);

    let arg0 = &arg_names[0]; // record
    let arg1 = &arg_names[1]; // old_record
    let arg2 = &arg_names[2]; // ctx

    let (impl_generics, _type_generics, where_clause) = fn_item.sig.generics.split_for_impl();

    let expanded = quote! {
        // The user's original function, renamed to a private inner.
        #[allow(non_snake_case)]
        // The inner fn's fixed SDK-typed signature may not mention the
        // caller's lifetimes; they are used only in the copied body.
        #[allow(clippy::extra_unused_lifetimes)]
        async fn #inner_name #impl_generics(
            #arg0: #sdk::Value,
            #arg1: ::core::option::Option<#sdk::Value>,
            #arg2: #sdk::Ctx,
        ) -> #sdk::Validation
        #where_clause
        {
            #fn_item
            #fn_name(#arg0, #arg1, #arg2).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        const _: () = { let _ = #inner_name; };

        /// Guest bump allocator: leak a `Vec<u8>` of `len` bytes, return its
        /// pointer. This memory is never freed — the WASM module is
        /// short-lived.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn shamir_alloc(len: i32) -> i32 {
            if len < 0 {
                #sdk::__rt::trap("invalid shamir_alloc length");
            }
            let v: ::std::vec::Vec<u8> = ::std::vec![0u8; len as usize];
            let ptr = v.as_ptr();
            if ptr as usize > ::core::primitive::i32::MAX as usize || (ptr as u64 + len as u64) > (1u64 << 31) {
                #sdk::__rt::trap("shamir_alloc outside guest ABI range");
            }
            ::core::mem::forget(v);
            ptr as i32
        }

        /// Guest ABI entry for a validator: decode params, extract record +
        /// old_record, drive the user's async function, encode the validation
        /// result, and return a packed `(ptr, len)`.
        /// # Safety
        /// `ptr` must be the guest-memory address of `len` initialized bytes
        /// readable for this call; the host must provide the allocation and
        /// keep it live until this function returns. Negative lengths are rejected.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub unsafe extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
            let end = ptr as i64 + len as i64;
            let memory_bytes = ::core::arch::wasm32::memory_size(0) as u64 * 65536;
            if ptr < 0 || len < 0 || (len > 0 && ptr == 0)
                || end as u64 > (1u64 << 31) || end as u64 > memory_bytes
            {
                #sdk::__rt::trap("invalid shamir_call pointer or length");
            }
            let bytes: &[u8] = if len == 0 {
                &[]
            } else {
                // SAFETY: caller upholds the pointer validity contract above; the
                // guards establish a positive length and non-null pointer.
                unsafe { ::core::slice::from_raw_parts(ptr as *const u8, len as usize) }
            };

            let params = match #sdk::__rt::decode_params(bytes) {
                ::core::result::Result::Ok(p) => p,
                ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
            };

            let mut entries = params.into_raw();
            let record: #sdk::Value = match entries.iter().position(|(key, _)| key == "record") {
                ::core::option::Option::Some(index) => entries.remove(index).1,
                ::core::option::Option::None => #sdk::__rt::raise(&#sdk::Error::codec("validator: missing `record` parameter")),
            };

            // Extract `old_record` (optional: absent or null → None).
            let old_record: ::core::option::Option<#sdk::Value> =
                match entries.iter().position(|(key, _)| key == "old_record") {
                    ::core::option::Option::Some(index) => match entries.remove(index).1 {
                        #sdk::Value::Null => ::core::option::Option::None,
                        value => ::core::option::Option::Some(value),
                    },
                    ::core::option::Option::None => ::core::option::Option::None,
                };
            ::core::mem::drop(entries);

            let ctx = #sdk::Ctx::new();

            let validation: #sdk::Validation =
                match #sdk::__rt::block_on(#inner_name(record, old_record, ctx)) {
                    ::core::result::Result::Ok(validation) => validation,
                    ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
                };

            let value = validation.into_value();
            let encoded = match #sdk::__rt::encode_value(&value) {
                ::core::result::Result::Ok(bytes) => bytes,
                ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
            };
            #sdk::__rt::leak_result(encoded)
        }
    };

    Ok(expanded)
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
pub fn function(attr: TokenStream, item: TokenStream) -> TokenStream {
    let SdkPath(sdk) = parse_macro_input!(attr as SdkPath);
    let fn_item = parse_macro_input!(item as ItemFn);
    match expand_function(fn_item, &sdk) {
        Ok(expanded) => expanded.into(),
        Err(err) => compile_error(err),
    }
}

fn expand_function(fn_item: ItemFn, sdk: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    let fn_name = &fn_item.sig.ident;
    let inner_name = helper_ident(fn_name);
    validate_signature(
        &fn_item,
        "function",
        3,
        matches!(&fn_item.sig.output, ReturnType::Type(_, ty) if is_result_value_return(ty)),
    )?;
    let arg_names = wrapper_idents(&fn_item);

    let arg0 = &arg_names[0]; // ctx
    let arg1 = &arg_names[1]; // batch
    let arg2 = &arg_names[2]; // params

    let (impl_generics, _type_generics, where_clause) = fn_item.sig.generics.split_for_impl();

    let expanded = quote! {
        // The user's original function, renamed to a private inner.
        #[allow(non_snake_case)]
        // The inner fn's fixed SDK-typed signature may not mention the
        // caller's lifetimes; they are used only in the copied body.
        #[allow(clippy::extra_unused_lifetimes)]
        async fn #inner_name #impl_generics(
            #arg0: #sdk::Ctx,
            #arg1: #sdk::Batch,
            #arg2: #sdk::Params,
        ) -> #sdk::Result<#sdk::Value>
        #where_clause
        {
            #fn_item
            #fn_name(#arg0, #arg1, #arg2).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        const _: () = { let _ = #inner_name; };

        /// Guest bump allocator: leak a `Vec<u8>` of `len` bytes, return its
        /// pointer. This memory is never freed — the WASM module is
        /// short-lived.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn shamir_alloc(len: i32) -> i32 {
            if len < 0 {
                #sdk::__rt::trap("invalid shamir_alloc length");
            }
            let v: ::std::vec::Vec<u8> = ::std::vec![0u8; len as usize];
            let ptr = v.as_ptr();
            if ptr as usize > ::core::primitive::i32::MAX as usize || (ptr as u64 + len as u64) > (1u64 << 31) {
                #sdk::__rt::trap("shamir_alloc outside guest ABI range");
            }
            ::core::mem::forget(v);
            ptr as i32
        }

        /// Guest ABI entry: decode params, drive the user's async function,
        /// encode the result, and return a packed `(ptr, len)`.
        ///
        /// On user `Err(...)` and malformed input: reports the error kind and
        /// message to the host. Invalid ABI pointers and lengths still trap.
        /// # Safety
        /// `ptr` must be the guest-memory address of `len` initialized bytes
        /// readable for this call; the host must provide the allocation and
        /// keep it live until this function returns. Negative lengths are rejected.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub unsafe extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
            let end = ptr as i64 + len as i64;
            let memory_bytes = ::core::arch::wasm32::memory_size(0) as u64 * 65536;
            if ptr < 0 || len < 0 || (len > 0 && ptr == 0)
                || end as u64 > (1u64 << 31) || end as u64 > memory_bytes
            {
                #sdk::__rt::trap("invalid shamir_call pointer or length");
            }
            let bytes: &[u8] = if len == 0 {
                &[]
            } else {
                // SAFETY: caller upholds the pointer validity contract above; the
                // guards establish a positive length and non-null pointer.
                unsafe { ::core::slice::from_raw_parts(ptr as *const u8, len as usize) }
            };

            let params = match #sdk::__rt::decode_params(bytes) {
                ::core::result::Result::Ok(p) => p,
                ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
            };
            let ctx = #sdk::Ctx::new();
            let batch = #sdk::Batch::new();

            let outcome: #sdk::Result<#sdk::Result<#sdk::Value>> =
                #sdk::__rt::block_on(#inner_name(ctx, batch, params));

            match outcome {
                ::core::result::Result::Ok(result) => match result {
                    ::core::result::Result::Ok(value) => {
                        let encoded = match #sdk::__rt::encode_value(&value) {
                            ::core::result::Result::Ok(bytes) => bytes,
                            ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
                        };
                        #sdk::__rt::leak_result(encoded)
                    }
                    ::core::result::Result::Err(e) => {
                        #sdk::__rt::raise(&e)
                    }
                },
                ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
            }
        }
    };

    Ok(expanded)
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
///     let rows = ctx.db().table("users").query(None)?;
///     Ok(Value::List(rows))
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
pub fn procedure(attr: TokenStream, item: TokenStream) -> TokenStream {
    let SdkPath(sdk) = parse_macro_input!(attr as SdkPath);
    let fn_item = parse_macro_input!(item as ItemFn);
    match expand_procedure(fn_item, &sdk) {
        Ok(expanded) => expanded.into(),
        Err(err) => compile_error(err),
    }
}

fn expand_procedure(fn_item: ItemFn, sdk: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    let fn_name = &fn_item.sig.ident;
    let inner_name = helper_ident(fn_name);
    validate_signature(
        &fn_item,
        "procedure",
        2,
        matches!(&fn_item.sig.output, ReturnType::Type(_, ty) if is_result_value_return(ty)),
    )?;
    let arg_names = wrapper_idents(&fn_item);
    let arg0 = &arg_names[0]; // ctx
    let arg1 = &arg_names[1]; // params

    let (impl_generics, _type_generics, where_clause) = fn_item.sig.generics.split_for_impl();

    let expanded = quote! {
        // The user's original function, renamed to a private inner.
        #[allow(non_snake_case)]
        // The inner fn's fixed SDK-typed signature may not mention the
        // caller's lifetimes; they are used only in the copied body.
        #[allow(clippy::extra_unused_lifetimes)]
        async fn #inner_name #impl_generics(
            #arg0: #sdk::Ctx,
            #arg1: #sdk::Params,
        ) -> #sdk::Result<#sdk::Value>
        #where_clause
        {
            #fn_item
            #fn_name(#arg0, #arg1).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        const _: () = { let _ = #inner_name; };

        /// Guest bump allocator: leak a `Vec<u8>` of `len` bytes, return its
        /// pointer. This memory is never freed — the WASM module is
        /// short-lived.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn shamir_alloc(len: i32) -> i32 {
            if len < 0 {
                #sdk::__rt::trap("invalid shamir_alloc length");
            }
            let v: ::std::vec::Vec<u8> = ::std::vec![0u8; len as usize];
            let ptr = v.as_ptr();
            if ptr as usize > ::core::primitive::i32::MAX as usize || (ptr as u64 + len as u64) > (1u64 << 31) {
                #sdk::__rt::trap("shamir_alloc outside guest ABI range");
            }
            ::core::mem::forget(v);
            ptr as i32
        }

        /// Guest ABI entry for a procedure: decode params, build Ctx, drive
        /// the user's async function, encode the result, and return a packed
        /// `(ptr, len)`.
        /// # Safety
        /// `ptr` must be the guest-memory address of `len` initialized bytes
        /// readable for this call; the host must provide the allocation and
        /// keep it live until this function returns. Negative lengths are rejected.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub unsafe extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
            let end = ptr as i64 + len as i64;
            let memory_bytes = ::core::arch::wasm32::memory_size(0) as u64 * 65536;
            if ptr < 0 || len < 0 || (len > 0 && ptr == 0)
                || end as u64 > (1u64 << 31) || end as u64 > memory_bytes
            {
                #sdk::__rt::trap("invalid shamir_call pointer or length");
            }
            let bytes: &[u8] = if len == 0 {
                &[]
            } else {
                // SAFETY: caller upholds the pointer validity contract above; the
                // guards establish a positive length and non-null pointer.
                unsafe { ::core::slice::from_raw_parts(ptr as *const u8, len as usize) }
            };

            let params = match #sdk::__rt::decode_params(bytes) {
                ::core::result::Result::Ok(p) => p,
                ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
            };
            let ctx = #sdk::Ctx::new();

            let outcome: #sdk::Result<#sdk::Result<#sdk::Value>> =
                #sdk::__rt::block_on(#inner_name(ctx, params));

            match outcome {
                ::core::result::Result::Ok(result) => match result {
                    ::core::result::Result::Ok(value) => {
                        let encoded = match #sdk::__rt::encode_value(&value) {
                            ::core::result::Result::Ok(bytes) => bytes,
                            ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
                        };
                        #sdk::__rt::leak_result(encoded)
                    }
                    ::core::result::Result::Err(e) => {
                        #sdk::__rt::raise(&e)
                    }
                },
                ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
            }
        }
    };

    Ok(expanded)
}

/// Returns `true` if the return type structurally matches `Result<Value>` in
/// any qualification form accepted by the SDK: bare `Result<Value>` (via
/// import), `shamir_sdk::Result<shamir_sdk::Value>`, `crate::Result<...>`,
/// `::std::result::Result<Value, Error>`, `core::result::Result<...>`, and
/// the rooted spellings of each. Any leading path prefix is accepted — the
/// compiler is the real type guard; this only produces a clear message.
fn is_result_value_return(ty: &Type) -> bool {
    let Type::Path(tp) = ty else { return false };
    let Some(seg) = tp.path.segments.last() else {
        return false;
    };
    if seg.ident != "Result" {
        return false;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return false;
    };
    let mut it = args.args.iter();
    let ok_ty = it.next().is_some_and(ga_ends_with("Value"));
    match args.args.len() {
        // `Result<Value>` (SDK alias) or `Result<Value, Error>`.
        1 => ok_ty,
        2 => ok_ty && it.next().is_some_and(ga_ends_with("Error")),
        _ => false,
    }
}

/// Returns `true` if the return type structurally matches `Validation` in any
/// qualification form (`Validation`, `shamir_sdk::Validation`,
/// `::shamir_sdk::Validation`, `crate::Validation`).
fn is_validation_return(ty: &Type) -> bool {
    let Type::Path(tp) = ty else { return false };
    let Some(seg) = tp.path.segments.last() else {
        return false;
    };
    seg.ident == "Validation" && matches!(&seg.arguments, PathArguments::None)
}

/// Closure checking that a generic argument's final path segment is `ident`.
fn ga_ends_with(ident: &'static str) -> impl Fn(&syn::GenericArgument) -> bool {
    move |ga: &syn::GenericArgument| matches!(ga, syn::GenericArgument::Type(ty) if last_ident_is_inner(ty, ident))
}

/// True if `ty`'s final path segment is `ident`.
fn last_ident_is_inner(ty: &Type, ident: &str) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == ident))
}

/// Returns `true` if the type token-stream contains `Ctx` anywhere.
///
/// Used by `#[scalar]` as a best-effort signature diagnostic; aliases are not resolved.
fn type_contains_ctx(ty: &Type) -> bool {
    let s = quote!(#ty).to_string();
    // Match standalone `Ctx` or fully-qualified `shamir_sdk::Ctx`, `crate::Ctx`
    // but not substrings like `CtxFoo`. We check word boundary via simple rules:
    // split on non-alphanumeric and check for exact "Ctx" segment.
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|segment| segment == "Ctx")
}

/// Attribute macro for a **scalar** — a guest function receiving `Params`.
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
/// there is no injected `Ctx`. A body may construct `Ctx`; this is not a
/// purity or security boundary, and host authorization still applies.
/// The generated wrapper forwards concrete `Params`, so Rust type checking,
/// not lexical matching, determines signature compatibility.
///
/// Compile-time checks:
/// - Must be `async`.
/// - Exactly 1 argument: `(params: Params)`.
/// - Return type must be `Result<Value>`.
/// - **No argument type may contain `Ctx`** — compile error if it does.
///   Use `#[procedure]` for functions that need database access.
#[proc_macro_attribute]
pub fn scalar(attr: TokenStream, item: TokenStream) -> TokenStream {
    let SdkPath(sdk) = parse_macro_input!(attr as SdkPath);
    let fn_item = parse_macro_input!(item as ItemFn);
    match expand_scalar(fn_item, &sdk) {
        Ok(expanded) => expanded.into(),
        Err(err) => compile_error(err),
    }
}

fn expand_scalar(fn_item: ItemFn, sdk: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    let fn_name = &fn_item.sig.ident;
    let inner_name = helper_ident(fn_name);
    validate_signature(
        &fn_item,
        "scalar",
        1,
        matches!(&fn_item.sig.output, ReturnType::Type(_, ty) if is_result_value_return(ty)),
    )?;
    let arg_names = wrapper_idents(&fn_item);
    for arg in &fn_item.sig.inputs {
        if let FnArg::Typed(PatType { ty, .. }) = arg {
            if type_contains_ctx(ty) {
                return Err(syn::Error::new_spanned(
                    ty,
                    "#[scalar] must not take Ctx; its argument must accept Params. Use #[procedure] for an injected Ctx.",
                ));
            }
        }
    }

    let arg0 = &arg_names[0]; // params

    let (impl_generics, _type_generics, where_clause) = fn_item.sig.generics.split_for_impl();

    let expanded = quote! {
        // The user's original function, renamed to a private inner.
        #[allow(non_snake_case)]
        // The inner fn's fixed SDK-typed signature may not mention the
        // caller's lifetimes; they are used only in the copied body.
        #[allow(clippy::extra_unused_lifetimes)]
        async fn #inner_name #impl_generics(
            #arg0: #sdk::Params,
        ) -> #sdk::Result<#sdk::Value>
        #where_clause
        {
            #fn_item
            #fn_name(#arg0).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        const _: () = { let _ = #inner_name; };

        /// Guest bump allocator: leak a `Vec<u8>` of `len` bytes, return its
        /// pointer. This memory is never freed — the WASM module is
        /// short-lived.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn shamir_alloc(len: i32) -> i32 {
            if len < 0 {
                #sdk::__rt::trap("invalid shamir_alloc length");
            }
            let v: ::std::vec::Vec<u8> = ::std::vec![0u8; len as usize];
            let ptr = v.as_ptr();
            if ptr as usize > ::core::primitive::i32::MAX as usize || (ptr as u64 + len as u64) > (1u64 << 31) {
                #sdk::__rt::trap("shamir_alloc outside guest ABI range");
            }
            ::core::mem::forget(v);
            ptr as i32
        }

        /// Guest ABI entry for a scalar: decode params, drive the user's
        /// async function, encode the result, and return a packed `(ptr, len)`.
        /// # Safety
        /// `ptr` must be the guest-memory address of `len` initialized bytes
        /// readable for this call; the host must provide the allocation and
        /// keep it live until this function returns. Negative lengths are rejected.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub unsafe extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
            let end = ptr as i64 + len as i64;
            let memory_bytes = ::core::arch::wasm32::memory_size(0) as u64 * 65536;
            if ptr < 0 || len < 0 || (len > 0 && ptr == 0)
                || end as u64 > (1u64 << 31) || end as u64 > memory_bytes
            {
                #sdk::__rt::trap("invalid shamir_call pointer or length");
            }
            let bytes: &[u8] = if len == 0 {
                &[]
            } else {
                // SAFETY: caller upholds the pointer validity contract above; the
                // guards establish a positive length and non-null pointer.
                unsafe { ::core::slice::from_raw_parts(ptr as *const u8, len as usize) }
            };

            let params = match #sdk::__rt::decode_params(bytes) {
                ::core::result::Result::Ok(p) => p,
                ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
            };

            let outcome: #sdk::Result<#sdk::Result<#sdk::Value>> =
                #sdk::__rt::block_on(#inner_name(params));

            match outcome {
                ::core::result::Result::Ok(result) => match result {
                    ::core::result::Result::Ok(value) => {
                        let encoded = match #sdk::__rt::encode_value(&value) {
                            ::core::result::Result::Ok(bytes) => bytes,
                            ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
                        };
                        #sdk::__rt::leak_result(encoded)
                    }
                    ::core::result::Result::Err(e) => {
                        #sdk::__rt::raise(&e)
                    }
                },
                ::core::result::Result::Err(e) => #sdk::__rt::raise(&e),
            }
        }
    };

    Ok(expanded)
}
