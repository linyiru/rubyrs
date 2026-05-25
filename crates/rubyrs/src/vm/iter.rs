//! Block-form Enumerable iterators. Mirrors CRuby's `enum.c` —
//! the home for the iterator drivers that walk Array / Hash /
//! Range while yielding to a block.
//!
//! Contents:
//!   - `IterMode` enum + helpers (filter-driver modes).
//!   - `Vm::iter_array_filter` / `iter_hash_filter` /
//!     `iter_range_filter` — shared drivers for select / reject /
//!     find / any? / all? / none?.
//!   - `Vm::collection_call_block` — the big match that
//!     dispatches every block-taking method (each / map /
//!     partition / sort_by / inject / flat_map / chunk / etc.)
//!     over every receiver type.

use crate::error::Trap;
use crate::heap::HeapObj;
use crate::value::{ObjId, Value};

use super::{value_cmp_v, PinGuard, Vm};


/// Which Enumerable predicate-iterator a call dispatches to.
/// `NoneM` is named with a trailing M because `None` collides with
/// `Option::None` in match arms.
#[derive(Copy, Clone, Debug)]
pub(crate) enum IterMode { Select, Reject, Find, Any, All, NoneM }

impl IterMode {
    fn bool_init(self) -> bool {
        // For `all?` we start at true and flip to false on first
        // falsy; for `none?` likewise. `any?` starts false.
        match self {
            IterMode::Any => false,
            IterMode::All | IterMode::NoneM => true,
            _ => false,
        }
    }
}

impl Vm {

    /// Drives an iterator-with-predicate over an Array. Used by
    /// `select` / `reject` / `find` / `any?` / `all?` / `none?`.
    /// On `break val` (caught via `self.break_signaled`) returns `val`
    /// to match CRuby's "break value short-circuits the enumerator".
    pub(crate) fn iter_array_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Value, Trap> {
        let snapshot: Vec<Value> = self.heap.array(id).clone();
        let mut g = PinGuard::new(self);
        g.pin(Value::Array(id));
        // P2-13: block lives in the GC heap; pin it for the
        // duration of the iteration so any GC fired by the block
        // body doesn't sweep it.
        g.pin(Value::Block(block));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
            g.pin(Value::Array(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        for v in snapshot {
            g.vm.invoke_block(block,vec![v.clone()])?;
            g.vm.dispatch_until(pre_frames)?;
            if g.vm.method_return.is_some() { break; }
            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
            if g.vm.break_signaled {
                g.vm.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(v); }
                IterMode::Reject => if !truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(v); }
                IterMode::Find => if truthy { find_val = v; break; }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
        }
        // PinGuard drops at function exit, including the `?` paths above.
        if let Some(e) = early { return Ok(e); }
        Ok(match mode {
            IterMode::Select | IterMode::Reject => Value::Array(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
        })
    }

    /// Same shape as `iter_array_filter`, but the source is a Hash.
    /// The block receives two args (key, value). `select`/`reject`
    /// return a Hash; `find` returns a `[k, v]` two-element Array (or nil).
    pub(crate) fn iter_hash_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Value, Trap> {
        let snapshot: Vec<(Value, Value)> = self.heap.hash(id).clone();
        let mut g = PinGuard::new(self);
        g.pin(Value::Hash(id));
        g.pin(Value::Block(block));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Hash(Vec::new()));
            g.pin(Value::Hash(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        for (k, v) in snapshot {
            g.vm.invoke_block(block,vec![k.clone(), v.clone()])?;
            g.vm.dispatch_until(pre_frames)?;
            if g.vm.method_return.is_some() { break; }
            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
            if g.vm.break_signaled {
                g.vm.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.hash_mut(acc_id.unwrap()).push((k, v)); }
                IterMode::Reject => if !truthy { g.vm.heap.hash_mut(acc_id.unwrap()).push((k, v)); }
                IterMode::Find => if truthy {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    find_val = Value::Array(pair);
                    break;
                }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
        }
        if let Some(e) = early { return Ok(e); }
        Ok(match mode {
            IterMode::Select | IterMode::Reject => Value::Hash(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
        })
    }

    /// Same shape as `iter_array_filter`, but iterates an Int Range.
    /// Returns `None` (Option) to the caller if the range's endpoints
    /// aren't both Ints — callers fall through to NoMethodError.
    pub(crate) fn iter_range_filter(&mut self, id: ObjId, mode: IterMode, block: ObjId) -> Result<Option<Value>, Trap> {
        let (bi, ei, excl) = {
            let r = self.heap.range(id);
            match (&r.begin, &r.end) {
                (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                _ => return Ok(None),
            }
        };
        let mut g = PinGuard::new(self);
        g.pin(Value::Range(id));
        g.pin(Value::Block(block));
        let acc_id = if matches!(mode, IterMode::Select | IterMode::Reject) {
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let rid = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
            g.pin(Value::Array(rid));
            Some(rid)
        } else { None };
        let pre_frames = g.vm.frames.len();
        let mut early: Option<Value> = None;
        let mut find_val = Value::Nil;
        let mut bool_acc = mode.bool_init();
        let end_inc = if excl { ei - 1 } else { ei };
        let mut i = bi;
        while i <= end_inc {
            g.vm.invoke_block(block,vec![Value::Int(i)])?;
            g.vm.dispatch_until(pre_frames)?;
            if g.vm.method_return.is_some() { break; }
            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
            if g.vm.break_signaled {
                g.vm.break_signaled = false;
                early = Some(r);
                break;
            }
            let truthy = r.is_truthy();
            match mode {
                IterMode::Select => if truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(Value::Int(i)); }
                IterMode::Reject => if !truthy { g.vm.heap.array_mut(acc_id.unwrap()).push(Value::Int(i)); }
                IterMode::Find => if truthy { find_val = Value::Int(i); break; }
                IterMode::Any => if truthy { bool_acc = true; break; }
                IterMode::All => if !truthy { bool_acc = false; break; }
                IterMode::NoneM => if truthy { bool_acc = false; break; }
            }
            i += 1;
        }
        if let Some(e) = early { return Ok(Some(e)); }
        Ok(Some(match mode {
            IterMode::Select | IterMode::Reject => Value::Array(acc_id.unwrap()),
            IterMode::Find => find_val,
            IterMode::Any | IterMode::All | IterMode::NoneM => Value::Bool(bool_acc),
        }))
    }

    pub(crate) fn collection_call_block(&mut self, recv: &Value, name: &str, args: &[Value], block: ObjId) -> Result<Option<Value>, Trap> {
        // Object#tap / #then / #yield_self — universal block
        // helpers. Yield `self` to the block; `tap` discards the
        // result and returns self (debug-style fluent chain),
        // `then` (and its `yield_self` alias) returns whatever
        // the block returned (Kleisli-style transform).
        if args.is_empty() && matches!(name, "tap" | "then" | "yield_self") {
            let pre_frames = self.frames.len();
            let mut g = PinGuard::new(self);
            g.pin(recv.clone());
            g.pin(Value::Block(block));
            g.vm.invoke_block(block, vec![recv.clone()])?;
            g.vm.dispatch_until(pre_frames)?;
            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
            return Ok(Some(if name == "tap" { recv.clone() } else { r }));
        }
        // `s.gsub(/pat/) { |m| ... }` / `s.sub(/pat/) { |m| ... }`.
        // For each match the block is invoked with the matched
        // substring; its return value is converted to a string and
        // spliced in place of the match. gsub iterates all matches;
        // sub does only the first. Backref groups in the matched
        // text are NOT exposed to the block — only the full match —
        // matching CRuby's "block gets the match string, not the
        // MatchData" convention for the common case.
        if let (Value::Str(s), Value::Regex(re), 1) = (recv, args.first().unwrap_or(&Value::Nil), args.len())
            && (name == "gsub" || name == "sub") {
                let source = s.to_string_lossy();
                let only_first = name == "sub";
                let mut g = PinGuard::new(self);
                g.pin(recv.clone());
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut out = String::with_capacity(source.len());
                let mut last_end = 0usize;
                let mut bail = false;
                for m in re.find_iter(&source) {
                    out.push_str(&source[last_end..m.start()]);
                    g.vm.invoke_block(block, vec![Value::new_str(m.as_str().to_string())])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { bail = true; break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        // CRuby semantics: break val from inside a
                        // gsub block returns val as the call's
                        // result (not the partially-built string).
                        return Ok(Some(r));
                    }
                    let r_str = r.to_display(&g.vm.heap, &g.vm.interner);
                    out.push_str(&r_str);
                    last_end = m.end();
                    if only_first { break; }
                }
                if bail { return Ok(None); }
                out.push_str(&source[last_end..]);
                return Ok(Some(Value::new_str(out)));
            }
        // `s.scan(/pat/) { |m| ... }` / `s.scan(string) { |m| ... }`
        // — yield each match to the block (capture-group Array if
        // the regex has groups, the matched substring otherwise).
        // Returns the receiver String, matching CRuby.
        if let (Value::Str(s), 1) = (recv, args.len()) && name == "scan" {
            let source: Vec<u8> = s.borrow().clone();
            let source_str = String::from_utf8_lossy(&source).into_owned();
            let mut g = PinGuard::new(self);
            g.pin(recv.clone());
            g.pin(Value::Block(block));
            let pre_frames = g.vm.frames.len();
            let mut early: Option<Value> = None;
            match &args[0] {
                Value::Regex(re) => {
                    let has_groups = re.captures_len() > 1;
                    if has_groups {
                        for caps in re.captures_iter(&source_str) {
                            let mut group_vec: Vec<Value> = Vec::with_capacity(caps.len() - 1);
                            for i in 1..caps.len() {
                                let v = caps.get(i)
                                    .map(|m| Value::new_str(m.as_str()))
                                    .unwrap_or(Value::Nil);
                                group_vec.push(v);
                            }
                            g.vm.maybe_gc();
                            g.vm.check_alloc()?;
                            let gid = g.vm.heap.alloc(HeapObj::Array(group_vec));
                            g.vm.invoke_block(block, vec![Value::Array(gid)])?;
                            g.vm.dispatch_until(pre_frames)?;
                            if g.vm.method_return.is_some() { return Ok(None); }
                            let _ = g.vm.stack.pop();
                            if g.vm.break_signaled {
                                g.vm.break_signaled = false;
                                early = Some(g.vm.stack.pop().unwrap_or(Value::Nil));
                                break;
                            }
                        }
                    } else {
                        for m in re.find_iter(&source_str) {
                            g.vm.invoke_block(block, vec![Value::new_str(m.as_str())])?;
                            g.vm.dispatch_until(pre_frames)?;
                            if g.vm.method_return.is_some() { return Ok(None); }
                            let _ = g.vm.stack.pop();
                            if g.vm.break_signaled {
                                g.vm.break_signaled = false;
                                early = Some(g.vm.stack.pop().unwrap_or(Value::Nil));
                                break;
                            }
                        }
                    }
                }
                Value::Str(pat) => {
                    let pat_owned: Vec<u8> = pat.borrow().clone();
                    if !pat_owned.is_empty() {
                        let bytes: &[u8] = &source;
                        let pat_bytes: &[u8] = &pat_owned;
                        let plen = pat_bytes.len();
                        let mut i = 0;
                        while i + plen <= bytes.len() {
                            if &bytes[i..i + plen] == pat_bytes {
                                g.vm.invoke_block(block, vec![Value::new_str_bytes(pat_owned.clone())])?;
                                g.vm.dispatch_until(pre_frames)?;
                                if g.vm.method_return.is_some() { return Ok(None); }
                                let _ = g.vm.stack.pop();
                                if g.vm.break_signaled {
                                    g.vm.break_signaled = false;
                                    early = Some(g.vm.stack.pop().unwrap_or(Value::Nil));
                                    break;
                                }
                                i += plen;
                            } else {
                                i += 1;
                            }
                        }
                    }
                }
                _ => return Ok(None),
            }
            return Ok(Some(early.unwrap_or_else(|| recv.clone())));
        }
        Ok(match (recv, name, args) {
            (Value::Array(id), "each", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block,vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Array(*id)))
            }
            (Value::Array(id), "map", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block,vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    g.vm.heap.array_mut(result_id).push(r);
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `flat_map { ... }` = map then flatten(1). Same
            // driver as map, but each block result that's an
            // Array gets spread into the result.
            // `arr.filter_map { |x| ... }` — map + select in one
            // pass. Block return is kept iff truthy; nil/false are
            // dropped (not "false is included" — strict truthiness,
            // matching CRuby).
            (Value::Array(id), "filter_map", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { return Ok(None); }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() {
                        g.vm.heap.array_mut(result_id).push(r);
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            (Value::Array(id), "flat_map", []) | (Value::Array(id), "collect_concat", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    match r {
                        Value::Array(rid) => {
                            let items: Vec<Value> = g.vm.heap.array(rid).clone();
                            for it in items { g.vm.heap.array_mut(result_id).push(it); }
                        }
                        other => g.vm.heap.array_mut(result_id).push(other),
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `chunk { |x| key }` groups consecutive elements
            // sharing the same key. Returns
            // `[[key, [vals...]], ...]`. nil/false key drops the
            // run from the output (matching CRuby's "skip" rule).
            (Value::Array(id), "chunk", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    // CRuby's chunk treats `nil` (and `:_separator`)
                    // as a drop-and-break sentinel. `false` is a
                    // normal key — its run shows up in the output.
                    // `:_alone` would also be special but is rare;
                    // we don't model it (documented divergence).
                    if matches!(key, Value::Nil) {
                        continue;
                    }
                    let same_as_last = groups.last()
                        .map(|(k, _)| k.ruby_eq(&key, &g.vm.heap))
                        .unwrap_or(false);
                    if same_as_last {
                        groups.last_mut().unwrap().1.push(v);
                    } else {
                        groups.push((key, vec![v]));
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let mut out: Vec<Value> = Vec::with_capacity(groups.len());
                for (key, items) in groups {
                    let items_id = g.vm.heap.alloc(HeapObj::Array(items));
                    g.pin(Value::Array(items_id));
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![key, Value::Array(items_id)]));
                    g.pin(Value::Array(pair_id));
                    out.push(Value::Array(pair_id));
                }
                let oid = g.vm.heap.alloc(HeapObj::Array(out));
                Some(Value::Array(oid))
            }
            (Value::Hash(id), "each", []) | (Value::Hash(id), "each_pair", []) => {
                // CRuby yields each pair as a single 2-elem Array
                // `[k, v]`. Two-param blocks (`|k, v|`) auto-splat
                // it into k / v; single-destructure blocks
                // (`|(k, v)|`) receive the pair and unpack via the
                // F4 compile-time prologue. Yielding two separate
                // args would defeat the destructure path because
                // the block's anonymous slot would get just `k`,
                // not the pair Array.
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.vm.invoke_block(block, vec![Value::Array(pair_id)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            (Value::Hash(id), "each_with_index", []) => {
                // Block invocation per CRuby: `(pair, idx)` where
                // `pair` is the fresh `[k, v]` Array. The block
                // running with a single param gets `pair` (an
                // Array). Two-param destructured form
                // (`|pair, idx|`) is what users usually want.
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (i, (k, v)) in snapshot.into_iter().enumerate() {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    g.pin(Value::Array(pair_id));
                    g.vm.invoke_block(block, vec![Value::Array(pair_id), Value::Int(i as i64)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Hash(id)))
            }
            (Value::Hash(id), "map", []) | (Value::Hash(id), "collect", []) => {
                // `h.map { |k, v| ... }` — yields each (k, v) and
                // collects block return values into a new Array.
                // CRuby returns an `Enumerator` for no-block, which
                // we don't have; falls through to NoMethodError if
                // misused that way.
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.invoke_block(block, vec![k, v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    g.vm.heap.array_mut(result_id).push(r);
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            // `h.transform_keys { |k| ... }` — new Hash with keys
            // mapped through the block. Values preserved. On
            // collision (block maps two distinct keys to the same
            // new key), later wins, matching CRuby.
            // `h.filter_map { |k, v| ... }` — yields each (k, v),
            // collects truthy block results into a fresh Array.
            // Like Array#filter_map but on Hash entries; the
            // result is NOT a Hash (CRuby behaviour).
            (Value::Hash(id), "filter_map", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.invoke_block(block, vec![k, v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { return Ok(None); }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() {
                        g.vm.heap.array_mut(result_id).push(r);
                    }
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }
            (Value::Hash(id), "transform_keys", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Hash(Vec::new()));
                g.pin(Value::Hash(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.invoke_block(block, vec![k])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { return Ok(None); }
                    let new_key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(new_key);
                        break;
                    }
                    // Last-wins collision: overwrite existing slot
                    // if the new_key equals one already present;
                    // otherwise append. Matches CRuby's iteration-
                    // order semantics.
                    let existing = g.vm.heap.hash(result_id).iter()
                        .position(|(k2, _)| k2.ruby_eq(&new_key, &g.vm.heap));
                    if let Some(p) = existing {
                        g.vm.heap.hash_mut(result_id)[p] = (new_key, v);
                    } else {
                        g.vm.heap.hash_mut(result_id).push((new_key, v));
                    }
                }
                Some(early.unwrap_or(Value::Hash(result_id)))
            }
            // `h.transform_values { |v| ... }` — new Hash with the
            // same keys but values mapped through the block. No
            // collision possible (keys unchanged); order preserved.
            (Value::Hash(id), "transform_values", []) => {
                let id = *id;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                let snapshot: Vec<(Value, Value)> = g.vm.heap.hash(id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Hash(Vec::with_capacity(snapshot.len())));
                g.pin(Value::Hash(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (k, v) in snapshot {
                    g.vm.invoke_block(block, vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { return Ok(None); }
                    let new_v = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(new_v);
                        break;
                    }
                    g.vm.heap.hash_mut(result_id).push((k, new_v));
                }
                Some(early.unwrap_or(Value::Hash(result_id)))
            }
            (Value::Hash(id), "fetch", [k]) => {
                // Block form: `h.fetch(k) { |k| default_expr }`.
                // Block is invoked only on miss; CRuby ignores the
                // 2-arg fetch + block combo (warns); we silently
                // accept it (handled in non-block path too).
                let id = *id;
                let pos = self.heap.hash(id).iter()
                    .position(|(key, _)| key.ruby_eq(k, &self.heap));
                if let Some(p) = pos {
                    return Ok(Some(self.heap.hash(id)[p].1.clone()));
                }
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(id));
                g.pin(Value::Block(block));
                g.pin(k.clone());
                let pre_frames = g.vm.frames.len();
                g.vm.invoke_block(block, vec![k.clone()])?;
                g.vm.dispatch_until(pre_frames)?;
                if g.vm.method_return.is_some() { return Ok(Some(Value::Nil)); }
                let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                Some(r)
            }
            (Value::Int(start), "upto", [Value::Int(stop)]) => {
                let start = *start;
                let stop = *stop;
                let pre_frames = self.frames.len();
                let mut early = None;
                let mut i = start;
                while i <= stop {
                    self.invoke_block(block,vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    if self.method_return.is_some() { break; }
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Int(start)))
            }
            (Value::Int(start), "downto", [Value::Int(stop)]) => {
                let start = *start;
                let stop = *stop;
                let pre_frames = self.frames.len();
                let mut early = None;
                let mut i = start;
                while i >= stop {
                    self.invoke_block(block,vec![Value::Int(i)])?;
                    self.dispatch_until(pre_frames)?;
                    if self.method_return.is_some() { break; }
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    if self.break_signaled {
                        self.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    i -= 1;
                }
                Some(early.unwrap_or(Value::Int(start)))
            }
            (Value::Int(n), "times", []) => {
                // Pin the block: the body may allocate freely (eg.
                // `N.times { a = [1,2,3] }`) which can trigger GC,
                // and the block ObjId is no longer on the stack at
                // this point — without a pin, the block slot gets
                // swept mid-iteration and the next invoke_block
                // panics with use-after-free (heap.rs:115).
                // Matches the pin pattern used by Array#each /
                // Range#each / map etc.
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let n_val = *n;
                for i in 0..n_val {
                    g.vm.invoke_block(block, vec![Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Int(n_val)))
            }
            (Value::Range(id), "each", []) => {
                // Two endpoint shapes drive iteration: Int+Int (the
                // common case, integer counting) and Str+Str (the
                // alphabetic `('a'..'z').each` case, driven by
                // String#succ). Other shapes fall through to
                // NoMethodError.
                let (b, e, excl) = {
                    let r = self.heap.range(*id);
                    (r.begin.clone(), r.end.clone(), r.exclusive)
                };
                match (&b, &e) {
                    (Value::Int(a), Value::Int(c)) => {
                        let bi = *a; let ei = *c;
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Range(*id));
                        g.pin(Value::Block(block));
                        let pre_frames = g.vm.frames.len();
                        let mut early = None;
                        let end_inc = if excl { ei - 1 } else { ei };
                        let mut i = bi;
                        while i <= end_inc {
                            g.vm.invoke_block(block,vec![Value::Int(i)])?;
                            g.vm.dispatch_until(pre_frames)?;
                            if g.vm.method_return.is_some() { break; }
                            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                            if g.vm.break_signaled {
                                g.vm.break_signaled = false;
                                early = Some(r);
                                break;
                            }
                            i += 1;
                        }
                        Some(early.unwrap_or(Value::Range(*id)))
                    }
                    (Value::Str(_), Value::Str(_)) => {
                        // Walk via String#succ, comparing
                        // lexicographically — matches CRuby's
                        // ('a'..'z').each iteration model.
                        let start = if let Value::Str(s) = &b { s.to_string_lossy() } else { unreachable!() };
                        let stop = if let Value::Str(s) = &e { s.to_string_lossy() } else { unreachable!() };
                        let mut g = PinGuard::new(self);
                        g.pin(Value::Range(*id));
                        g.pin(Value::Block(block));
                        let pre_frames = g.vm.frames.len();
                        let mut early = None;
                        let mut cur = start;
                        loop {
                            // Inclusive: stop when cur > stop. Exclusive: stop when cur >= stop.
                            let done = if excl { cur >= stop } else { cur > stop };
                            if done { break; }
                            g.vm.invoke_block(block, vec![Value::new_str(cur.clone())])?;
                            g.vm.dispatch_until(pre_frames)?;
                            if g.vm.method_return.is_some() { break; }
                            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                            if g.vm.break_signaled {
                                g.vm.break_signaled = false;
                                early = Some(r);
                                break;
                            }
                            // succ; if the result is longer than `stop`, no further
                            // String less-than-or-equal can be true with our lex
                            // ordering — bail to avoid an unbounded loop for cases
                            // like ('a'..'9') where succ rolls into a longer string.
                            let next = super::string::str_succ(&cur);
                            if next.len() > stop.len() { break; }
                            cur = next;
                        }
                        Some(early.unwrap_or(Value::Range(*id)))
                    }
                    _ => return Ok(None),
                }
            }
            (Value::Array(id), "each_with_index", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for (i, v) in snapshot.into_iter().enumerate() {
                    g.vm.invoke_block(block,vec![v, Value::Int(i as i64)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or(Value::Array(*id)))
            }
            (Value::Array(id), "each_with_object", [seed]) => {
                // `arr.each_with_object(memo) { |elem, memo| ... }`.
                // CRuby threads `memo` unchanged across iterations
                // (unlike inject which uses the block's return as the
                // next accumulator). The block's return value is
                // ignored; users mutate `memo` for side effects.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                g.pin(seed.clone());
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v, seed.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                }
                Some(early.unwrap_or_else(|| seed.clone()))
            }
            (Value::Array(id), "partition", []) => {
                // `arr.partition { |x| pred(x) }` returns
                // `[truthy_array, falsy_array]` — exactly two new
                // Arrays. Used a lot in routing / grouping idioms.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let yes_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(yes_id));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let no_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(no_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() {
                        g.vm.heap.array_mut(yes_id).push(v);
                    } else {
                        g.vm.heap.array_mut(no_id).push(v);
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let pair_id = g.vm.heap.alloc(HeapObj::Array(vec![
                    Value::Array(yes_id), Value::Array(no_id),
                ]));
                Some(Value::Array(pair_id))
            }
            // `arr.take_while { |x| ... }` / `#drop_while` — prefix
            // partitioning. `take_while` returns the prefix while
            // the block is truthy and stops at the first falsy
            // return. `drop_while` skips that prefix and returns
            // the rest unchanged (block isn't invoked past the
            // crossing point).
            (Value::Array(id), "take_while" | "drop_while", []) => {
                let want_take = name == "take_while";
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early: Option<Value> = None;
                let mut crossed = false;
                let mut crossing_idx: Option<usize> = None;
                for (i, v) in snapshot.iter().enumerate() {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { return Ok(None); }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if !r.is_truthy() {
                        crossed = true;
                        crossing_idx = Some(i);
                        break;
                    }
                    if want_take {
                        g.vm.heap.array_mut(result_id).push(v.clone());
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                if !want_take {
                    // drop_while: copy from the crossing point to
                    // the end. If we never crossed, drop_while
                    // returns []; the result_id is already empty.
                    if let Some(start) = crossing_idx {
                        for w in &snapshot[start..] {
                            g.vm.heap.array_mut(result_id).push(w.clone());
                        }
                    } else if !crossed {
                        // Block was truthy for every element →
                        // drop_while drops the whole array. Already
                        // empty result_id is correct.
                    }
                }
                Some(Value::Array(result_id))
            }
            // `arr.bsearch { |x| ... }` — binary search a sorted
            // Array. Two modes, distinguished by the block's return
            // type at runtime:
            //   find-minimum (block returns Bool / nil): returns the
            //     smallest element for which the block is truthy,
            //     nil if none. Array must be partitioned false...true.
            //   find-any (block returns Int): 0 = match, <0 means
            //     "x too large" (search left), >0 means "x too
            //     small" (search right). Returns the matching
            //     element or nil. Array must be sorted in the
            //     comparison direction.
            // Other block-return types raise TypeError, matching
            // CRuby.
            (Value::Array(id), "bsearch", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut low = 0usize;
                let mut high = snapshot.len();
                let mut saw_int = false;
                let mut int_match: Option<Value> = None;
                while low < high {
                    let mid = low + (high - low) / 2;
                    let elem = snapshot[mid].clone();
                    g.vm.invoke_block(block, vec![elem.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { return Ok(None); }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        return Ok(Some(r));
                    }
                    match r {
                        Value::Bool(true) => high = mid,
                        Value::Bool(false) | Value::Nil => low = mid + 1,
                        Value::Int(n) => {
                            saw_int = true;
                            if n == 0 { int_match = Some(elem); break; }
                            else if n < 0 { high = mid; }
                            else { low = mid + 1; }
                        }
                        other => return Err(g.vm.trap(crate::error::RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (must be numeric, true, false or nil)",
                                other.type_name(),
                            ),
                        })),
                    }
                }
                if let Some(m) = int_match { return Ok(Some(m)); }
                if saw_int { return Ok(Some(Value::Nil)); }
                Some(if low < snapshot.len() { snapshot[low].clone() } else { Value::Nil })
            }
            // `arr.chunk_while { |a, b| pred(a, b) }` — partition
            // into runs of consecutive elements where the block
            // returns truthy for the pair (a=prev, b=current).
            // Falsy starts a new chunk. Empty input → `[]`;
            // single-element → `[[elem]]`.
            (Value::Array(id), "chunk_while", []) => {
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                if snapshot.is_empty() {
                    return Ok(Some(Value::Array(result_id)));
                }
                let pre_frames = g.vm.frames.len();
                let mut current_chunk: Vec<Value> = vec![snapshot[0].clone()];
                let mut early: Option<Value> = None;
                for pair in snapshot.windows(2) {
                    g.vm.invoke_block(block, vec![pair[0].clone(), pair[1].clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { return Ok(None); }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() {
                        current_chunk.push(pair[1].clone());
                    } else {
                        // Flush current chunk and start a fresh one.
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let chunk_id = g.vm.heap.alloc(HeapObj::Array(std::mem::take(&mut current_chunk)));
                        g.vm.heap.array_mut(result_id).push(Value::Array(chunk_id));
                        current_chunk.push(pair[1].clone());
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                // Flush the trailing chunk.
                if !current_chunk.is_empty() {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let chunk_id = g.vm.heap.alloc(HeapObj::Array(current_chunk));
                    g.vm.heap.array_mut(result_id).push(Value::Array(chunk_id));
                }
                Some(Value::Array(result_id))
            }
            // `arr.min_by(n) { |x| key(x) }` / `arr.max_by(n) { ... }`
            // — top-n form. Returns an Array of `n` extremes
            // sorted by key (ascending for min_by, descending for
            // max_by). `n <= 0` yields `[]`; `n > len` yields all
            // elements sorted. Uses a full sort_by then truncate,
            // not a heap — O(n log n) is fine at the input sizes
            // we see in our niche.
            (Value::Array(id), "min_by", [Value::Int(n)])
            | (Value::Array(id), "max_by", [Value::Int(n)]) => {
                let want_min = name == "min_by";
                if *n < 0 {
                    return Err(self.trap(crate::error::RubyError::ArgumentError {
                        msg: format!("negative size ({})", n),
                    }));
                }
                let n_take = *n as usize;
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::new()));
                g.pin(Value::Array(result_id));
                if n_take == 0 || snapshot.is_empty() {
                    return Ok(Some(Value::Array(result_id)));
                }
                let pre_frames = g.vm.frames.len();
                let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(snapshot.len());
                let mut early: Option<Value> = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { return Ok(None); }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    pairs.push((key, v));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                let interner = &g.vm.interner;
                // CRuby treats incomparable keys here as an error;
                // we keep the same shape as sort_by — return None
                // and let the caller surface NoMethodError.
                let mut incomparable = false;
                pairs.sort_by(|(ka, _), (kb, _)| {
                    match value_cmp_v(ka, kb, interner) {
                        Some(o) => o,
                        None => { incomparable = true; std::cmp::Ordering::Equal }
                    }
                });
                if incomparable { return Ok(None); }
                let take = n_take.min(pairs.len());
                let result_vec: Vec<Value> = if want_min {
                    pairs.into_iter().take(take).map(|(_, v)| v).collect()
                } else {
                    // Largest n: reverse-sorted prefix.
                    pairs.into_iter().rev().take(take).map(|(_, v)| v).collect()
                };
                *g.vm.heap.array_mut(result_id) = result_vec;
                Some(Value::Array(result_id))
            }
            (Value::Array(id), "min_by", []) | (Value::Array(id), "max_by", []) => {
                // For each element, call the block once to produce a
                // key. Track the running winner. Returns nil for an
                // empty array (matching CRuby). Block-keys that
                // aren't mutually comparable surface as NoMethodError
                // via `value_cmp_v` returning None for one of them —
                // same shape as sort_by.
                let want_min = name == "min_by";
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                if snapshot.is_empty() { return Ok(Some(Value::Nil)); }
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let mut best: Option<(Value, Value)> = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    best = Some(match best {
                        None => (key, v),
                        Some((bk, bv)) => match value_cmp_v(&key, &bk, &g.vm.interner) {
                            Some(std::cmp::Ordering::Less) if want_min => (key, v),
                            Some(std::cmp::Ordering::Greater) if !want_min => (key, v),
                            // Equal or wrong direction — keep prior.
                            Some(_) => (bk, bv),
                            // Incomparable keys — fall through to None below.
                            None => return Ok(None),
                        },
                    });
                }
                if let Some(e) = early { return Ok(Some(e)); }
                Some(best.map(|(_, v)| v).unwrap_or(Value::Nil))
            }
            (Value::Array(id), "group_by", []) => {
                // Group elements into a Hash keyed by the block's
                // return value. Insertion order matches first
                // appearance of each key — CRuby semantics. Values
                // collect into a fresh Array per key.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let snapshot: Vec<Value> = g.vm.heap.array(*id).clone();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let result_id = g.vm.heap.alloc(HeapObj::Hash(Vec::new()));
                g.pin(Value::Hash(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    // Find or create the bucket array for this key.
                    let pos = g.vm.heap.hash(result_id).iter()
                        .position(|(k, _)| k.ruby_eq(&key, &g.vm.heap));
                    if let Some(p) = pos {
                        if let Value::Array(arr_id) = g.vm.heap.hash(result_id)[p].1 {
                            g.vm.heap.array_mut(arr_id).push(v);
                        }
                    } else {
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let arr_id = g.vm.heap.alloc(HeapObj::Array(vec![v]));
                        g.vm.heap.hash_mut(result_id).push((key, Value::Array(arr_id)));
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                Some(Value::Hash(result_id))
            }
            (Value::Array(id), "sort_by", []) => {
                // PinGuard wraps the entire impl — the previous code
                // dropped the guard after the key-collection loop,
                // leaving `pairs` (a Rust local) to carry ObjId-
                // bearing element Values through `user_cmp` insertion
                // sort and the trailing `maybe_gc()` with no GC root.
                // Symptom: `.to_a.sort_by` chains where the receiver
                // Array of pairs has no other anchor → pair Arrays
                // swept → dangling slots in the result.
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let arr = g.vm.heap.array(*id).clone();
                let pre_frames = g.vm.frames.len();
                let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(arr.len());
                let mut early: Option<Value> = None;
                for v in arr {
                    g.vm.invoke_block(block, vec![v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    g.pin(key.clone());
                    g.pin(v.clone());
                    pairs.push((key, v));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                let n = pairs.len();
                for i in 1..n {
                    let mut j = i;
                    while j > 0 {
                        let (k_prev, k_curr) = {
                            let (a, b) = pairs.split_at(j);
                            (a[j - 1].0.clone(), b[0].0.clone())
                        };
                        let ord = g.vm.user_cmp(&k_prev, &k_curr)?;
                        match ord {
                            None => return Ok(None),
                            Some(std::cmp::Ordering::Greater) => {
                                pairs.swap(j - 1, j);
                                j -= 1;
                            }
                            _ => break,
                        }
                    }
                }
                let sorted: Vec<Value> = pairs.into_iter().map(|(_, v)| v).collect();
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let nid = g.vm.heap.alloc(HeapObj::Array(sorted));
                Some(Value::Array(nid))
            }
            (Value::Array(id), "inject", []) | (Value::Array(id), "reduce", []) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                if snapshot.is_empty() { return Ok(Some(Value::Nil)); }
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = snapshot[0].clone();
                let mut early = None;
                for v in &snapshot[1..] {
                    g.vm.invoke_block(block,vec![acc.clone(), v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Array(id), "inject", [init]) | (Value::Array(id), "reduce", [init]) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = init.clone();
                let mut early = None;
                for v in &snapshot {
                    g.vm.invoke_block(block,vec![acc.clone(), v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Array(id), "count", []) => {
                let snapshot: Vec<Value> = self.heap.array(*id).clone();
                let mut g = PinGuard::new(self);
                g.pin(Value::Array(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                for v in snapshot {
                    g.vm.invoke_block(block,vec![v])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() { n += 1; }
                }
                Some(early.unwrap_or(Value::Int(n)))
            }
            (Value::Range(id), "inject", []) | (Value::Range(id), "reduce", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                if bi > end_inc { return Ok(Some(Value::Nil)); }
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = Value::Int(bi);
                let mut early = None;
                let mut i = bi + 1;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![acc.clone(), Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                    i += 1;
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Range(id), "inject", [init]) | (Value::Range(id), "reduce", [init]) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut acc = init.clone();
                let mut early = None;
                let mut i = bi;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![acc.clone(), Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    acc = r;
                    i += 1;
                }
                Some(early.unwrap_or(acc))
            }
            (Value::Range(id), "count", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                let mut n: i64 = 0;
                let mut early = None;
                let mut i = bi;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    if r.is_truthy() { n += 1; }
                    i += 1;
                }
                Some(early.unwrap_or(Value::Int(n)))
            }

            (Value::Array(id), "select", []) | (Value::Array(id), "filter", []) => Some(self.iter_array_filter(*id, IterMode::Select, block)?),
            (Value::Array(id), "reject", []) => Some(self.iter_array_filter(*id, IterMode::Reject, block)?),
            (Value::Array(id), "find", []) | (Value::Array(id), "detect", []) => Some(self.iter_array_filter(*id, IterMode::Find, block)?),
            (Value::Array(id), "any?", []) => Some(self.iter_array_filter(*id, IterMode::Any, block)?),
            (Value::Array(id), "all?", []) => Some(self.iter_array_filter(*id, IterMode::All, block)?),
            (Value::Array(id), "none?", []) => Some(self.iter_array_filter(*id, IterMode::NoneM, block)?),

            // Hash#min_by / #max_by — yield (k, v) to the block,
            // pick the pair whose block-returned key is the
            // extremum. Result is the winning [k, v] as a fresh
            // 2-element Array, matching CRuby. Empty hash → nil.
            (Value::Hash(id), op @ ("min_by" | "max_by"), []) => {
                let want_max = op == "max_by";
                let pairs: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                if pairs.is_empty() { return Ok(Some(Value::Nil)); }
                let mut best: Option<(Value, Value, Value)> = None;
                let mut early: Option<Value> = None;
                {
                    let mut g = PinGuard::new(self);
                    g.pin(Value::Hash(*id));
                    g.pin(Value::Block(block));
                    let pre_frames = g.vm.frames.len();
                    for (k, v) in pairs {
                        g.vm.invoke_block(block, vec![k.clone(), v.clone()])?;
                        g.vm.dispatch_until(pre_frames)?;
                        if g.vm.method_return.is_some() { break; }
                        let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                        if g.vm.break_signaled {
                            g.vm.break_signaled = false;
                            early = Some(key);
                            break;
                        }
                        best = match best {
                            None => Some((k, v, key)),
                            Some((bk, bv, bkey)) => {
                                let ord = match value_cmp_v(&key, &bkey, &g.vm.interner) {
                                    Some(o) => o,
                                    None => return Ok(None),
                                };
                                let want_replace = if want_max {
                                    ord == std::cmp::Ordering::Greater
                                } else {
                                    ord == std::cmp::Ordering::Less
                                };
                                if want_replace { Some((k, v, key)) }
                                else { Some((bk, bv, bkey)) }
                            }
                        };
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                if let Some((k, v, _)) = best {
                    // PinGuard the winning pair across the explicit
                    // `maybe_gc`: previously k/v were Rust locals
                    // with no root, so STRESS_GC could sweep them
                    // before the new Array was alloc'd → dangling
                    // ObjIds inside the result.
                    let mut g = PinGuard::new(self);
                    g.pin(k.clone());
                    g.pin(v.clone());
                    g.vm.maybe_gc();
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    Some(Value::Array(pid))
                } else {
                    Some(Value::Nil)
                }
            }

            // Hash#sort_by — yield (k, v), use returned key as the
            // sort key, return an Array of [k, v] pairs in key
            // order. Stability preserved via insertion sort.
            (Value::Hash(id), "sort_by", []) => {
                // PinGuard wraps the *entire* impl, not just the
                // block-invocation phase. Previously the guard
                // dropped before the post-loop `maybe_gc`, leaving
                // `keyed` (a Rust local) holding ObjId-bearing
                // Values with no GC root → STRESS_GC swept them and
                // the resulting Array<[k,v]> had dangling slots
                // that exploded inside `to_display`.
                let pairs_in: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                let mut keyed: Vec<(Value, Value, Value)> = Vec::with_capacity(pairs_in.len());
                let mut early: Option<Value> = None;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                for (k, v) in pairs_in {
                    g.vm.invoke_block(block, vec![k.clone(), v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let key = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(key);
                        break;
                    }
                    // Pin each accumulated triple component so the
                    // next iter's invoke_block (which may GC) can't
                    // sweep them.
                    g.pin(key.clone());
                    g.pin(k.clone());
                    g.pin(v.clone());
                    keyed.push((key, k, v));
                }
                if let Some(e) = early { return Ok(Some(e)); }
                let n = keyed.len();
                for i in 1..n {
                    let mut j = i;
                    while j > 0 {
                        let ord = {
                            let a = keyed[j - 1].0.clone();
                            let b = keyed[j].0.clone();
                            g.vm.user_cmp(&a, &b)?
                        };
                        match ord {
                            None => return Ok(None),
                            Some(std::cmp::Ordering::Greater) => {
                                keyed.swap(j - 1, j);
                                j -= 1;
                            }
                            _ => break,
                        }
                    }
                }
                g.vm.maybe_gc();
                let mut out: Vec<Value> = Vec::with_capacity(keyed.len());
                for (_, k, v) in keyed {
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    let pv = Value::Array(pid);
                    g.pin(pv.clone());
                    out.push(pv);
                }
                let oid = g.vm.heap.alloc(HeapObj::Array(out));
                Some(Value::Array(oid))
            }

            // Hash#group_by — bucket pairs by the block's return.
            // Each bucket is an Array of [k, v] pairs; the result
            // is a Hash from group-key → Array.
            (Value::Hash(id), "group_by", []) => {
                // Same GC root-hole pattern as sort_by above: the
                // previous impl scoped PinGuard only across the
                // block invocation, then dropped it and ran more
                // alloc work (with `maybe_gc`) over `buckets` and
                // each freshly-built pair Array. Extend the guard
                // and pin each new ObjId as it's created.
                let pairs_in: Vec<(Value, Value)> = self.heap.hash(*id).clone();
                let mut buckets: Vec<(Value, Vec<Value>)> = Vec::new();
                let mut early: Option<Value> = None;
                let mut g = PinGuard::new(self);
                g.pin(Value::Hash(*id));
                g.pin(Value::Block(block));
                let pre_frames = g.vm.frames.len();
                for (k, v) in pairs_in {
                    g.vm.invoke_block(block, vec![k.clone(), v.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let group = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(group);
                        break;
                    }
                    let pid = g.vm.heap.alloc(HeapObj::Array(vec![k, v]));
                    let pair = Value::Array(pid);
                    g.pin(pair.clone());
                    g.pin(group.clone());
                    let pos = buckets.iter().position(|(gk, _)| gk.ruby_eq(&group, &g.vm.heap));
                    match pos {
                        Some(p) => buckets[p].1.push(pair),
                        None => buckets.push((group, vec![pair])),
                    }
                }
                if let Some(e) = early { return Ok(Some(e)); }
                g.vm.maybe_gc();
                let mut hash_pairs: Vec<(Value, Value)> = Vec::with_capacity(buckets.len());
                for (gk, vs) in buckets {
                    let aid = g.vm.heap.alloc(HeapObj::Array(vs));
                    let av = Value::Array(aid);
                    g.pin(av.clone());
                    hash_pairs.push((gk, av));
                }
                let hid = g.vm.heap.alloc(HeapObj::Hash(hash_pairs));
                Some(Value::Hash(hid))
            }

            (Value::Hash(id), "select", []) | (Value::Hash(id), "filter", []) => Some(self.iter_hash_filter(*id, IterMode::Select, block)?),
            (Value::Hash(id), "reject", []) => Some(self.iter_hash_filter(*id, IterMode::Reject, block)?),
            (Value::Hash(id), "find", []) | (Value::Hash(id), "detect", []) => Some(self.iter_hash_filter(*id, IterMode::Find, block)?),
            (Value::Hash(id), "any?", []) => Some(self.iter_hash_filter(*id, IterMode::Any, block)?),
            (Value::Hash(id), "all?", []) => Some(self.iter_hash_filter(*id, IterMode::All, block)?),
            (Value::Hash(id), "none?", []) => Some(self.iter_hash_filter(*id, IterMode::NoneM, block)?),

            (Value::Range(id), "select", []) | (Value::Range(id), "filter", []) => self.iter_range_filter(*id, IterMode::Select, block)?,
            (Value::Range(id), "reject", []) => self.iter_range_filter(*id, IterMode::Reject, block)?,
            (Value::Range(id), "find", []) | (Value::Range(id), "detect", []) => self.iter_range_filter(*id, IterMode::Find, block)?,
            (Value::Range(id), "any?", []) => self.iter_range_filter(*id, IterMode::Any, block)?,
            (Value::Range(id), "all?", []) => self.iter_range_filter(*id, IterMode::All, block)?,
            (Value::Range(id), "none?", []) => self.iter_range_filter(*id, IterMode::NoneM, block)?,

            (Value::Range(id), "map", []) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let mut g = PinGuard::new(self);
                g.pin(Value::Range(*id));
                g.pin(Value::Block(block));
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let count = if excl { (ei - bi).max(0) } else { (ei - bi + 1).max(0) };
                let result_id = g.vm.heap.alloc(HeapObj::Array(Vec::with_capacity(count as usize)));
                g.pin(Value::Array(result_id));
                let pre_frames = g.vm.frames.len();
                let mut early = None;
                let end_inc = if excl { ei - 1 } else { ei };
                let mut i = bi;
                while i <= end_inc {
                    g.vm.invoke_block(block,vec![Value::Int(i)])?;
                    g.vm.dispatch_until(pre_frames)?;
                    if g.vm.method_return.is_some() { break; }
                    let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                    if g.vm.break_signaled {
                        g.vm.break_signaled = false;
                        early = Some(r);
                        break;
                    }
                    g.vm.heap.array_mut(result_id).push(r);
                    i += 1;
                }
                Some(early.unwrap_or(Value::Array(result_id)))
            }

            // Range Enumerable fallback: materialize as an Array
            // and re-dispatch through the Array arms above. This
            // gets each_with_index / each_with_object / partition
            // / min_by / max_by / group_by / sort_by "for free"
            // and keeps a single source of truth for the
            // iteration semantics. Cost: one Vec<Value::Int>
            // allocation. Only Int-bounded ranges (the common
            // case) qualify — heterogeneous ranges would need
            // their own dispatch.
            (Value::Range(id), name, args) if matches!(name,
                "each_with_index" | "each_with_object" |
                "partition" | "min_by" | "max_by" |
                "group_by" | "sort_by"
            ) => {
                let (bi, ei, excl) = {
                    let r = self.heap.range(*id);
                    match (&r.begin, &r.end) {
                        (Value::Int(a), Value::Int(c)) => (*a, *c, r.exclusive),
                        _ => return Ok(None),
                    }
                };
                let end_inc = if excl { ei - 1 } else { ei };
                let mut elems: Vec<Value> = Vec::new();
                let mut v = bi;
                while v <= end_inc {
                    elems.push(Value::Int(v));
                    v += 1;
                }
                // Pin the block AND every incoming arg FIRST: a
                // STRESS_GC pass triggered by `maybe_gc` below could
                // otherwise sweep the block-handle slot or an arg
                // value (e.g. the memo Hash passed to
                // `each_with_object({})`) — neither is necessarily
                // on the operand stack at this point, only borrowed
                // through `&[Value]` from the dispatch caller, which
                // doesn't count as a GC root. Symptoms were the
                // "ICE: heap slot is not a Block" and "is not a Hash"
                // panics in `range_enumerable`.
                let mut g = PinGuard::new(self);
                g.pin(Value::Block(block));
                for a in args { g.pin(a.clone()); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let arr_id = g.vm.heap.alloc(HeapObj::Array(elems));
                g.pin(Value::Array(arr_id));
                let arr_val = Value::Array(arr_id);
                return g.vm.collection_call_block(&arr_val, name, args, block);
            }
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Block-iteration short-circuit semantics — `break` /
    //! `next` / non-local `return` inside iter_array_filter
    //! and friends. These are integration-shape tests (drive
    //! `Runtime::eval` with small scripts and assert stdout)
    //! because the per-driver short-circuit machinery is
    //! deeply tied to dispatch / step, and unit-testing it in
    //! isolation would mean reconstructing most of the
    //! evaluator.
    //!
    //! The diff_cruby `enumerable_filter.rb` /
    //! `control_flow.rb` fixtures cover the end-to-end CRuby
    //! match; these tests pin the contract module-locally so a
    //! regression surfaces here first.
    use crate::{Config, Runtime};

    /// Configure a Runtime with stdout pointing at a shared
    /// in-memory buffer, run `src`, return the captured stdout.
    fn capture(src: &str) -> String {
        use std::sync::{Arc, Mutex};
        // Box<dyn Write> doesn't expose the inner buffer, so wrap
        // an Arc<Mutex<Vec<u8>>> behind a small adapter that
        // implements `io::Write` and clones the Arc.
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut rt = Runtime::with_config(Config::default());
        rt.set_stdout(Box::new(Sink(buf.clone())));
        let _ = rt.eval(src, "test.rb").expect("eval succeeded");
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn array_each_break_returns_break_value() {
        // `break` from inside Array#each makes the call return the
        // break-arg.
        let out = capture(r#"
            r = [1, 2, 3, 4].each { |x| break "stop at #{x}" if x == 2 }
            puts r
        "#);
        assert_eq!(out, "stop at 2\n");
    }

    #[test]
    fn array_each_break_no_value_returns_nil() {
        let out = capture(r#"
            r = [1, 2, 3].each { |x| break if x == 2 }
            puts r.nil?
        "#);
        assert_eq!(out, "true\n");
    }

    #[test]
    fn array_map_next_replaces_iteration_value() {
        // `next val` makes the current iteration's contribution be
        // `val` (Array#map collects what each block call returns).
        let out = capture(r#"
            r = [1, 2, 3].map { |x| next 0 if x == 2; x }
            p r
        "#);
        assert_eq!(out, "[1, 0, 3]\n");
    }

    #[test]
    fn array_select_break_truncates() {
        let out = capture(r#"
            r = [1, 2, 3, 4, 5].select { |x| break [-1] if x == 3; x.even? }
            p r
        "#);
        // Break short-circuits the select entirely, returning the
        // break value (not the partial selection).
        assert_eq!(out, "[-1]\n");
    }

    #[test]
    fn array_each_with_index_break_value() {
        let out = capture(r#"
            r = ["a", "b", "c"].each_with_index { |v, i| break i if v == "b" }
            puts r
        "#);
        assert_eq!(out, "1\n");
    }

    #[test]
    fn hash_each_next_continues() {
        // `next` (no value) just skips to the next iteration.
        let out = capture(r#"
            sum = 0
            { a: 1, b: 2, c: 3 }.each { |(_k, v)| next if v == 2; sum = sum + v }
            puts sum
        "#);
        assert_eq!(out, "4\n");
    }

    #[test]
    fn range_each_break_returns_value() {
        let out = capture(r#"
            r = (1..10).each { |i| break "early at #{i}" if i > 3 }
            puts r
        "#);
        assert_eq!(out, "early at 4\n");
    }

    #[test]
    fn nonlocal_return_from_block_exits_enclosing_method() {
        // `return` from inside a driver block escapes the entire
        // method, not just the block. Different from `break`,
        // which only exits the driver and returns to the method.
        let out = capture(r#"
            def find
              [1, 2, 3].each { |x| return "got #{x}" if x == 2 }
              "not found"
            end
            puts find
        "#);
        assert_eq!(out, "got 2\n");
    }
}
