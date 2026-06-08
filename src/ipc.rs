//! IPC bridge: register Rust host commands callable from JS via
//! `__TAURI__.invoke(cmd, ...args)`, `__TAURI__.cmd(...args)`, or
//! the v3.14 globally-bound `invoke(cmd, ...args)` shorthand.
//!
//! Limitations:
//!
//! - Commands are plain `NativeFn` function pointers (no captured
//!   state).  Host state must thread through the boa-cat heap.
//! - Synchronous calls only; async / Promise-returning commands wait
//!   on the comp-cat-rs scheduler integration in a later release.
//! - The thread-local `INVOKE_REGISTRY` that backs the globally-bound
//!   `invoke` is last-install-wins per thread.  Multiple
//!   `HostCommands::install` calls on the same thread overwrite the
//!   previous registry; the per-thread snapshot is fine for the
//!   single-threaded eval loop the runtime uses today.

use std::cell::RefCell;
use std::collections::BTreeMap;

use boa_cat::Value;
use boa_cat::env::Env;
use boa_cat::fuel::Fuel;
use boa_cat::heap::Heap;
use boa_cat::outcome::{EvalResult, Outcome};
use boa_cat::value::{Cell, NativeFn, Object};

thread_local! {
    /// Per-thread snapshot of the most-recently-installed
    /// `HostCommands` registry, consulted by [`global_invoke_impl`]
    /// when `invoke(cmd, ...args)` is called at the top level (i.e.
    /// without a `this` binding to `__TAURI__`).  Last-install-wins
    /// per thread; the single-threaded eval loop the runtime uses
    /// today never observes two registries at once.
    static INVOKE_REGISTRY: RefCell<Vec<(String, NativeFn)>> =
        const { RefCell::new(Vec::new()) };
}

/// A registry of host commands callable from JS.
///
/// Build with [`Self::new`] and chain [`Self::with`] for each command,
/// then call [`Self::install`] (or use one of the script driver
/// helpers in [`crate::script`]) to expose the commands to JS as the
/// `__TAURI__` global.
#[derive(Debug, Default, Clone)]
pub struct HostCommands {
    commands: Vec<(String, NativeFn)>,
}

impl HostCommands {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `callback` under `name`.  Returns a new registry; the
    /// original is consumed.
    #[must_use]
    pub fn with(self, name: impl Into<String>, callback: NativeFn) -> Self {
        let extended: Vec<(String, NativeFn)> = self
            .commands
            .into_iter()
            .chain(std::iter::once((name.into(), callback)))
            .collect();
        Self { commands: extended }
    }

    /// Install the registry into `env` / `heap` as the `__TAURI__`
    /// global plus a globally-bound `invoke(cmd, ...args)` shorthand
    /// (v3.14).  Each registered command becomes a property on the
    /// `__TAURI__` object so `__TAURI__.cmd(...)` and
    /// `__TAURI__.invoke('cmd', ...)` continue to work; the new
    /// top-level `invoke` cell additionally dispatches without a
    /// `this` binding via the thread-local registry, so callers can
    /// do `invoke('cmd', ...)` or `const i = invoke; i('cmd', ...)`.
    /// The commands snapshot is stored in [`INVOKE_REGISTRY`] for the
    /// global path to consult; the in-`__TAURI__` `invoke` method
    /// keeps its v0.5 `this`-based dispatch path.
    #[must_use]
    pub fn install(&self, env: Env, heap: Heap) -> (Env, Heap) {
        INVOKE_REGISTRY.with(|slot| {
            slot.replace(self.commands.clone());
        });
        let properties: BTreeMap<String, Value> = self
            .commands
            .iter()
            .map(|(name, callback)| (name.clone(), Value::Native(*callback)))
            .chain(std::iter::once((
                "invoke".to_owned(),
                Value::Native(invoke_impl),
            )))
            .collect();
        let (tauri_id, heap) = heap.alloc_object(Object::from_properties(properties));
        let (tauri_cell, heap) = heap.alloc_cell(Cell::new(Value::Object(tauri_id), false));
        let (invoke_cell, heap) =
            heap.alloc_cell(Cell::new(Value::Native(global_invoke_impl), false));
        let bindings = [("__TAURI__", tauri_cell), ("invoke", invoke_cell)];
        bindings
            .into_iter()
            .fold((env, heap), |(env, heap), (name, cell)| {
                (env.extend_cell(name, cell), heap)
            })
    }
}

/// v3.14 globally-bound `invoke(cmd, ...args)` dispatcher.  Looks
/// the command up in the per-thread [`INVOKE_REGISTRY`] rather than
/// on a `this` receiver, so plain `invoke("greet", "x")` works
/// regardless of how the binding was reached.  Returns `null` when
/// the command name is missing or unregistered.
///
/// # Errors
///
/// Propagates errors raised by the dispatched host command.
#[allow(clippy::needless_pass_by_value)]
fn global_invoke_impl(args: Vec<Value>, _this: Value, heap: Heap, fuel: Fuel) -> EvalResult {
    let command_name = first_string_arg(&args);
    let forwarded: Vec<Value> = args.into_iter().skip(1).collect();
    let callback_opt = INVOKE_REGISTRY.with(|slot| {
        slot.borrow()
            .iter()
            .find(|(name, _)| name == &command_name)
            .map(|(_, callback)| *callback)
    });
    callback_opt.map_or(
        Ok((Outcome::Normal(Value::Null), heap.clone(), fuel)),
        |callback| callback(forwarded, Value::Undefined, heap, fuel),
    )
}

fn first_string_arg(args: &[Value]) -> String {
    args.first()
        .and_then(|first| match first {
            Value::String(s) => Some(s.clone()),
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::Object(_)
            | Value::Function(_)
            | Value::Native(_) => None,
        })
        .unwrap_or_default()
}

/// `__TAURI__.invoke(cmd, ...args)` dispatcher.  Looks up the command
/// on the receiver object (the `__TAURI__` instance bound as `this`)
/// and forwards the remaining args.  Returns `null` if the command
/// name is missing or unregistered.
///
/// # Errors
///
/// Propagates errors raised by the dispatched host command.
#[allow(clippy::needless_pass_by_value)]
fn invoke_impl(args: Vec<Value>, this: Value, heap: Heap, fuel: Fuel) -> EvalResult {
    let command_name = first_string_arg(&args);
    let this_id = match &this {
        Value::Object(id) => Some(*id),
        Value::Undefined
        | Value::Null
        | Value::Boolean(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Function(_)
        | Value::Native(_) => None,
    };
    let command_fn = this_id
        .and_then(|id| heap.object(id))
        .and_then(|obj| obj.get(&command_name).cloned())
        .and_then(|value| match value {
            Value::Native(f) => Some(f),
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Object(_)
            | Value::Function(_) => None,
        });
    let forwarded: Vec<Value> = args.into_iter().skip(1).collect();
    command_fn.map_or(
        Ok((Outcome::Normal(Value::Null), heap.clone(), fuel)),
        |callback| callback(forwarded, this, heap, fuel),
    )
}
