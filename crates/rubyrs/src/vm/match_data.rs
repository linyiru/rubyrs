//! MatchData materialization — shared between `String#match`
//! (vm/string.rs) and the `$~` read path (vm/step.rs). Keeps one
//! source of truth for the @whole/@caps ivar shape, the
//! two-allocation cap accounting, and the "MatchData class not
//! loaded → nil" fallback. Cfg-gated on `regex` along with every
//! other consumer of `last_match`.
#![cfg(feature = "regex")]


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
    /// Named captures extracted from the regex. Each entry is
    /// `(name, Some(matched_string) | None)`. Non-participating
    /// named groups (alternation arms that didn't match) keep
    /// `None`, matching CRuby's contract that
    /// `named_captures["x"]` returns nil rather than `""`.
    pub(crate) named_captures: Vec<(String, Option<String>)>,
}

impl Vm {
    pub(crate) fn materialize_match_data_with_context(
        &mut self,
        whole: String,
        caps: Vec<Value>,
        ctx: MatchDataContext,
    ) -> Result<Value, Trap> {
        self.maybe_gc();
        self.check_alloc()?;
        let caps_arr = self.heap.alloc(HeapObj::Array(caps.into()));
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
            ivars: crate::intern::FxHashMap::default(),
            singleton_class: None,
            frozen: std::cell::Cell::new(false),
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
        // Named captures install as @named_caps: Hash<String,
        // String | nil>. The preamble's `MatchData#[]` consults
        // this hash for Symbol / String indexes; `#named_captures`
        // returns it directly. Empty Hash for unnamed-only
        // patterns — the `Vec` is empty in that case, so the
        // resulting Hash is also empty (matches CRuby).
        if !ctx.named_captures.is_empty() {
            let pairs: Vec<(Value, Value)> = ctx.named_captures
                .into_iter()
                .map(|(name, val)| {
                    let v = match val {
                        Some(s) => Value::new_str(s),
                        None => Value::Nil,
                    };
                    (Value::new_str(name), v)
                })
                .collect();
            self.check_alloc()?;
            let h_id = self.heap.alloc(HeapObj::Hash(
                crate::heap::HashObj::with_pairs(pairs)
            ));
            let nc_ivar = self.interner.intern("@named_caps");
            self.heap.instance_mut(obj_id).ivars.insert(nc_ivar, Value::Hash(h_id));
        }
        Ok(Value::Object(obj_id))
    }

    /// Materialize the current `$~` (`self.last_match`) into a full
    /// MatchData Value — including `#pre_match` / `#post_match` /
    /// `#string`, reconstructed from the stored input + match span.
    /// Returns nil when there is no last match. Shared by the `$~`
    /// global read and `Regexp.last_match` so both expose the same
    /// surface, including named-capture access (`$~[:name]`).
    pub(crate) fn materialize_last_match(&mut self) -> Result<Value, Trap> {
        let extracted = self.last_match.as_ref().map(|lm| {
            let caps: Vec<Value> = lm
                .caps
                .iter()
                .map(|c| match c {
                    Some(s) => Value::new_str(s.clone()),
                    None => Value::Nil,
                })
                .collect();
            let ctx = MatchDataContext {
                pre_match: lm.input.get(..lm.m_start).map(|s| s.to_string()),
                post_match: lm.input.get(lm.m_end..).map(|s| s.to_string()),
                string: Some(lm.input.clone()),
                regexp: None,
                named_captures: lm.named.clone(),
            };
            (lm.whole.clone(), caps, ctx)
        });
        match extracted {
            Some((whole, caps, ctx)) => {
                self.materialize_match_data_with_context(whole, caps, ctx)
            }
            None => Ok(Value::Nil),
        }
    }

    /// Run `re` against `bound`, set the `$~` side-channel, and return
    /// a materialised `MatchData` (or `Nil` on no match, which also
    /// clears `$~` — CRuby parity). Shared by `String#match` and
    /// `Regexp#match` so both expose identical capture / `$~`
    /// behaviour. Discovery: P3 Jekyll spike — kramdown's header
    /// parser does `HEADER_ID.match(text)` (Regexp receiver).
    pub(crate) fn do_regexp_match(
        &mut self,
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        bound: String,
    ) -> Result<Value, Trap> {
        let owned = re.captures_owned(&bound).map_err(|e| {
            self.trap(crate::error::RubyError::RuntimeError {
                msg: format!("regex match failed: {} (pattern: /{}/)", e, re.as_str()),
            })
        })?;
        match owned {
            None => {
                self.last_match = None;
                Ok(Value::Nil)
            }
            Some(oc) => {
                let pre = bound[..oc.m_start].to_string();
                let post = bound[oc.m_end..].to_string();
                let full_str = bound.clone();
                let group_vals: Vec<Value> = oc
                    .groups
                    .iter()
                    .map(|g| match g {
                        Some(s) => Value::new_str(s.clone()),
                        None => Value::Nil,
                    })
                    .collect();
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole.clone(),
                    caps: oc.groups.clone(),
                    input: bound,
                    m_start: oc.m_start,
                    m_end: oc.m_end,
                    named: oc.named.clone(),
                });
                let ctx = MatchDataContext {
                    pre_match: Some(pre),
                    post_match: Some(post),
                    string: Some(full_str),
                    regexp: Some(Value::Regex(re.clone())),
                    named_captures: oc.named,
                };
                self.materialize_match_data_with_context(oc.whole, group_vals, ctx)
            }
        }
    }
}
