//! Raw wasmtime harness for compiled `shamir-lang` modules: the guest ABI
//! (`memory`, `shamir_alloc`, `shamir_call`) is driven directly, so raw input
//! bytes go in and raw output bytes come out (`WasmFunction::call` goes
//! through `QueryValue` and cannot show either), and one instance serves
//! several calls (the second-call guard is observable).
//!
//! The `raise_error` host import mirrors `bindings/raise.rs`: kind, pointer
//! and length are validated, the message is read from guest memory, and the
//! call ends in a trap. The raise is also recorded in the store so tests
//! assert kind and message without depending on how wasmtime renders traps.

/// The last `raise_error` the guest made on a store.
#[derive(Default)]
pub(crate) struct RaiseLog {
    kind: Option<i32>,
    message: Vec<u8>,
}

/// A raw wasmtime instance of a compiled module.
pub(crate) struct RawModule {
    store: wasmtime::Store<RaiseLog>,
    memory: wasmtime::Memory,
    alloc: wasmtime::TypedFunc<i32, i32>,
    call: wasmtime::TypedFunc<(i32, i32), i64>,
}

/// Parse space-separated hex text into bytes so the design's vectors appear
/// verbatim in each test.
pub(crate) fn hex(bytes: &str) -> Vec<u8> {
    bytes
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).expect("hex byte"))
        .collect()
}

/// Instantiate `wasm` on a fresh engine. Mirrors the API surface of
/// `wasm_function.rs` (default engine, `instantiate_async`, `get_memory`,
/// `get_typed_func`) and the raise contract of `bindings/raise.rs`.
pub(crate) async fn raw_module(wasm: &[u8]) -> RawModule {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_binary(&engine, wasm).expect("compiled module parses");
    let mut linker = wasmtime::Linker::<RaiseLog>::new(&engine);
    linker
        .func_wrap(
            "shamir_host",
            "raise_error",
            |mut caller: wasmtime::Caller<'_, RaiseLog>,
             kind: i32,
             ptr: i32,
             len: i32|
             -> Result<(), wasmtime::Error> {
                if ptr < 0 || len < 0 {
                    return Err(wasmtime::Error::msg(
                        "raise_error: negative pointer or length",
                    ));
                }
                let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
                    return Err(wasmtime::Error::msg("raise_error: missing memory"));
                };
                let start = ptr as usize;
                let end = start.saturating_add(len as usize);
                let data = memory.data(&caller);
                if end > data.len() {
                    return Err(wasmtime::Error::msg(
                        "raise_error: guest memory out of bounds",
                    ));
                }
                let message = data[start..end].to_vec();
                let rendered = format!(
                    "guest raise kind {kind}: {}",
                    String::from_utf8_lossy(&message)
                );
                let log = caller.data_mut();
                log.kind = Some(kind);
                log.message = message;
                Err(wasmtime::Error::msg(rendered))
            },
        )
        .expect("raise_error import links");
    let mut store = wasmtime::Store::new(&engine, RaiseLog::default());
    let instance = linker
        .instantiate_async(&mut store, &module)
        .await
        .expect("module instantiates");
    let memory = instance
        .get_memory(&mut store, "memory")
        .expect("memory export");
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "shamir_alloc")
        .expect("alloc export");
    let call = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, "shamir_call")
        .expect("call export");
    RawModule {
        store,
        memory,
        alloc,
        call,
    }
}

impl RawModule {
    /// The kind and message of the guest's last `raise_error`, if any.
    pub(crate) fn raised(&self) -> Option<(i32, String)> {
        let log = self.store.data();
        log.kind
            .map(|kind| (kind, String::from_utf8_lossy(&log.message).into_owned()))
    }
}

/// Writes `input` into guest memory through `shamir_alloc`, calls
/// `shamir_call`, and returns the raw output bytes read back from memory.
/// Mirrors `wasm_function.rs`'s call path: the negative-pointer guard,
/// `data_size` bounds checks, `data_mut(&mut store)` slicing for the write
/// and `data(&store)` slicing for the read, and the packed
/// `(ptr << 32) | len` i64 unpack.
pub(crate) async fn raw_call(
    module: &mut RawModule,
    input: &[u8],
) -> Result<Vec<u8>, wasmtime::Error> {
    let len =
        i32::try_from(input.len()).map_err(|_| wasmtime::Error::msg("input too large for i32"))?;
    let ptr = module.alloc.call_async(&mut module.store, len).await?;
    if ptr < 0 {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned negative pointer",
        ));
    }
    let start = ptr as usize;
    let end = start.saturating_add(input.len());
    if end > module.memory.data_size(&module.store) {
        return Err(wasmtime::Error::msg(
            "shamir_alloc pointer outside memory bounds",
        ));
    }
    module.memory.data_mut(&mut module.store)[start..end].copy_from_slice(input);
    let packed = module
        .call
        .call_async(&mut module.store, (ptr, len))
        .await?;
    let out_ptr = ((packed as u64) >> 32) as usize;
    let out_len = (packed & 0xFFFF_FFFF) as u32 as usize;
    let out_end = out_ptr.saturating_add(out_len);
    if out_end > module.memory.data_size(&module.store) {
        return Err(wasmtime::Error::msg("result pointer outside memory bounds"));
    }
    Ok(module.memory.data(&module.store)[out_ptr..out_end].to_vec())
}
