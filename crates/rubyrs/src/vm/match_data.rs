//! MatchData materialization — shared between `String#match`
//! (vm/string.rs) and the `$~` read path (vm/step.rs). Keeps one
//! source of truth for the @whole/@caps ivar shape, the
//! two-allocation cap accounting, and the "MatchData class not
//! loaded → nil" fallback. Cfg-gated on `regex` along with every
//! other consumer of `last_match`.
#![cfg(feature = "regex")]

use std::collections::HashMap;

use crate::error::Trap;
use crate::heap::HeapObj;
use crate::value::{Instance, Value};
use crate::vm::Vm;

/// Optional context bundled with a MatchData allocation. Every
/// field maps 1:1 to the matching `@ivar` set on the
/// `MatchData` instance — exposed via `#pre_match`, `#post_match`,
/// `#string`, `#regexp`. Call sites that don't have the data
/// pass `None`; the corresponding ivar is left at its initialize
/// default (nil), matching what the preamble's `initialize`
/// would produce on a Ruby-side allocation.
#[derive(Default)]
pub(crate) struct MatchDataContext {
    pub(crate) pre_match: Option<String>,
    pub(crate) post_match: Option<String>,
    pub(crate) string: Option<String>,
    pub(crate) regexp: Option<Value>,
}

impl Vm {
    pub(crate) fn materialize_match_data(
        &mut self,
        whole: String,
        caps: Vec<Value>,
    ) -> Result<Value, Trap> {
        self.materialize_match_data_with_context(whole, caps, MatchDataContext::default())
    }

    pub(crate) fn materialize_match_data_with_context(
        &mut self,
        whole: String,
        caps: Vec<Value>,
        ctx: MatchDataContext,
    ) -> Result<Value, Trap> {
        self.maybe_gc();
        self.check_alloc()?;
        let caps_arr = self.heap.alloc(HeapObj::Array(caps));
        let cls_id = self.interner.intern("MatchData");
        let cls = match self.classes.get(&cls_id).cloned() {
            Some(c) => c,
            None => return Ok(Value::Nil),
        };
        // Second alloc — re-check the cap so a tight `heap.max_live`
        // budget that admitted `caps_arr` but not the Instance traps
        // cleanly rather than sneaking past the limit.
        self.check_alloc()?;
        let obj_id = self.heap.alloc(HeapObj::Instance(Instance {
            class: cls,
            ivars: HashMap::new(),
            singleton_class: None,
        }));
        let whole_ivar = self.interner.intern("@whole");
        let caps_ivar = self.interner.intern("@caps");
        let inst = self.heap.instance_mut(obj_id);
        inst.ivars.insert(whole_ivar, Value::new_str(whole));
        inst.ivars.insert(caps_ivar, Value::Array(caps_arr));
        // Optional context ivars — only inserted when the caller
        // supplied the data. Absent ivars resolve to nil via the
        // standard ivar-read fallback, so the MatchData methods
        // (`#pre_match`, `#regexp`, ...) behave the same as if the
        // preamble's `initialize` ran with `nil` defaults.
        let pre_ivar = self.interner.intern("@pre_match");
        let post_ivar = self.interner.intern("@post_match");
        let str_ivar = self.interner.intern("@string");
        let re_ivar = self.interner.intern("@regexp");
        if let Some(s) = ctx.pre_match {
            self.heap.instance_mut(obj_id).ivars.insert(pre_ivar, Value::new_str(s));
        }
        if let Some(s) = ctx.post_match {
            self.heap.instance_mut(obj_id).ivars.insert(post_ivar, Value::new_str(s));
        }
        if let Some(s) = ctx.string {
            self.heap.instance_mut(obj_id).ivars.insert(str_ivar, Value::new_str(s));
        }
        if let Some(v) = ctx.regexp {
            self.heap.instance_mut(obj_id).ivars.insert(re_ivar, v);
        }
        Ok(Value::Object(obj_id))
    }
}
