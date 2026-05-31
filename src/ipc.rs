//! IPC bridge: register Rust host commands callable from JS via
//! `__TAURI__.invoke(cmd, ...args)` or directly via
//! `__TAURI__.cmd(...args)`.
//!
//! v0.5 limitations:
//!
//! - Commands are plain `NativeFn` function pointers (no captured
//!   state).  Host state must thread through the boa-cat heap.
//! - Synchronous calls only; async / Promise-returning commands wait
//!   on the comp-cat-rs scheduler integration in a later release.
//! - The dispatcher looks up the command on `this`, which means
//!   `__TAURI__.invoke(...)` only works as a method call.  Standalone
//!   `const invoke = __TAURI__.invoke; invoke("greet", "x")` will not
//!   resolve the command (because `this` becomes undefined).

use std::collections::BTreeMap;

use boa_cat::Value;
use boa_cat::env::Env;
use boa_cat::fuel::Fuel;
use boa_cat::heap::Heap;
use boa_cat::outcome::{EvalResult, Outcome};
use boa_cat::value::{Cell, NativeFn, Object};

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
    /// global.  Each registered command becomes a property on the
    /// `__TAURI__` object, plus an `invoke` dispatcher method.
    #[must_use]
    pub fn install(&self, env: Env, heap: Heap) -> (Env, Heap) {
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
        let (cell_id, heap) = heap.alloc_cell(Cell::new(Value::Object(tauri_id), false));
        std::iter::once(("__TAURI__", cell_id)).fold((env, heap), |(env, heap), (name, cell)| {
            (env.extend_cell(name, cell), heap)
        })
    }
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
    let command_name = args
        .first()
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
        .unwrap_or_default();
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
