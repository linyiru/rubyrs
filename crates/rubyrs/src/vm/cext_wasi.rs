//! wasm32-wasi alt for [`Vm::cext_require`]. WASI has no dynamic
//! loader, so `require "path/to/some.so"` from Ruby on wasi has no
//! way to succeed; we trap with a precise message instead of
//! silently returning Nil. Native targets get the dlopen-based
//! implementation in `vm/cext.rs` (which is `#![cfg(not(target_os
//! = "wasi"))]` at the top, so the two halves are mutually
//! exclusive). Only reachable through the cext feature's
//! kernel-side `require` arm; with the feature off, kernel.rs
//! emits its own no-cext error and never calls into here.
#![cfg(all(feature = "cext", target_os = "wasi"))]

use crate::error::{RubyError, Trap};
use crate::value::Value;
use crate::vm::Vm;

impl Vm {
    pub(crate) fn cext_require(&mut self, path_str: &str) -> Result<Value, Trap> {
        Err(self.trap(RubyError::RuntimeError {
            msg: format!(
                "require: C-ext loading is not supported on wasm32-wasi (attempted to load {})",
                path_str
            ),
        }))
    }
}
