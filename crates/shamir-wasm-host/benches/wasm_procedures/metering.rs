//! What the sandbox's metering costs: the same kernels on a bare engine with fuel and/or
//! epoch interruption switched on or off. Calls are synchronous and exclude instantiation.

use std::cell::RefCell;
use std::hint::black_box;
use std::rc::Rc;

use bench_scale_tool::Harness;
use wasmtime::{Config, Engine, Instance, Module, Store, TypedFunc};

use crate::kernels;

struct RawGuest {
    store: Store<()>,
    call: TypedFunc<(i32, i32), i64>,
}

/// Applies `SHAMIR_WASM_STRATEGY` the way the product engine does (see `bench_strategies`).
fn apply_strategy(config: &mut Config) -> Result<(), String> {
    #[cfg(feature = "bench-strategies")]
    match std::env::var("SHAMIR_WASM_STRATEGY").ok().as_deref() {
        Some("winch") => {
            config.strategy(wasmtime::Strategy::Winch);
        }
        Some("pulley") => {
            config
                .target("pulley64")
                .map_err(|error| error.to_string())?;
        }
        _ => {}
    }
    #[cfg(not(feature = "bench-strategies"))]
    let _ = config;
    Ok(())
}

fn build_guest(wat: &str, fuel: bool, epoch: bool) -> Result<RawGuest, String> {
    let mut config = Config::new();
    config.consume_fuel(fuel);
    config.epoch_interruption(epoch);
    apply_strategy(&mut config)?;
    let engine = Engine::new(&config).map_err(|error| error.to_string())?;
    let module = Module::new(&engine, wat).map_err(|error| error.to_string())?;
    let mut store = Store::new(&engine, ());
    if fuel {
        store
            .set_fuel(u64::MAX / 2)
            .map_err(|error| error.to_string())?;
    }
    if epoch {
        store.set_epoch_deadline(u64::MAX / 2);
    }
    let instance = Instance::new(&mut store, &module, &[]).map_err(|error| error.to_string())?;
    let call = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, "shamir_call")
        .map_err(|error| error.to_string())?;
    Ok(RawGuest { store, call })
}

pub fn register(h: &mut Harness) {
    let guests = [
        ("int_sum", kernels::int_sum_wat()),
        ("fib", kernels::fib_wat()),
    ];
    for (mode, fuel, epoch) in [
        ("fuel_epoch", true, true),
        ("fuel", true, false),
        ("epoch", false, true),
        ("none", false, false),
    ] {
        for (kernel, wat) in &guests {
            let guest = match build_guest(wat, fuel, epoch) {
                Ok(guest) => Rc::new(RefCell::new(guest)),
                Err(reason) => {
                    println!("metering/{mode}/{kernel}: skipped ({reason})");
                    continue;
                }
            };
            h.bench(&format!("metering/{mode}/{kernel}"), move || {
                let mut guard = guest.borrow_mut();
                let RawGuest { store, call } = &mut *guard;
                black_box(call.call(&mut *store, (0, 0)).expect("call"));
            });
        }
    }
}
