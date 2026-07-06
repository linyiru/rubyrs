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
    /// Byte spans of capture groups 1..N (full-`string` coordinates;
    /// `None` for a non-participating group). Installed as
    /// `@group_byte_offsets` so `MatchData#begin`/`#end`/`#offset` (+
    /// `byte*`) can resolve group indices. Empty when unavailable.
    pub(crate) group_offsets: Vec<Option<(usize, usize)>>,
    /// Names of groups 1..N in index order (parallel to `group_offsets`)
    /// — installed as `@cap_names` so `#begin(:name)` resolves a named
    /// index to its group position.
    pub(crate) cap_names: Vec<Option<String>>,
}

impl Vm {
    /// Build the frame-scoped `$~` side-channel from engine-agnostic
    /// owned captures. Used by `String#scan` (block and no-block) so
    /// native-regex and fancy-regex matches publish identical MatchData
    /// state: `$~`, `$1..`, `$&`, pre/post-match, named captures, and
    /// group offsets.
    pub(crate) fn last_match_from_owned_captures(
        &self,
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        input: &str,
        oc: &crate::regex_engine::OwnedCaptures,
    ) -> crate::vm::LastMatch {
        crate::vm::LastMatch {
            whole: oc.whole.clone(),
            caps: oc.groups.clone(),
            input: input.to_string(),
            m_start: oc.m_start,
            m_end: oc.m_end,
            named: oc.named.clone(),
            group_spans: oc.group_spans.clone(),
            cap_names: re.capture_group_names(),
            binary: None,
        }
    }

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
            ivars: crate::value::IvarTable::default(),
            singleton_class: None,
            frozen: std::cell::Cell::new(false),
        }));
        let whole_ivar = self.interner.intern("@whole");
        let caps_ivar = self.interner.intern("@caps");
        let inst = self.heap.instance_mut(obj_id);
        inst.ivar_set(whole_ivar, Value::new_str(whole));
        inst.ivar_set(caps_ivar, Value::Array(caps_arr));
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
            self.heap.instance_mut(obj_id).ivar_set(pre_ivar, Value::new_str(s));
        }
        if let Some(s) = ctx.post_match {
            self.heap.instance_mut(obj_id).ivar_set(post_ivar, Value::new_str(s));
        }
        if let Some(s) = ctx.string {
            self.heap.instance_mut(obj_id).ivar_set(str_ivar, Value::new_str(s));
        }
        if let Some(v) = ctx.regexp {
            self.heap.instance_mut(obj_id).ivar_set(re_ivar, v);
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
            self.heap.instance_mut(obj_id).ivar_set(nc_ivar, Value::Hash(h_id));
        }
        // Group byte spans (@group_byte_offsets) + group names
        // (@cap_names) back MatchData#begin/#end/#offset (+ byte*).
        // Each entry is a 2-element [begin, end] Array or nil. The
        // whole-match span (index 0) is derived in the preamble from
        // @pre_match / @whole, so only groups 1..N are stored here
        // (index 0 of these arrays = group 1). Skip installing when
        // empty so unmatched/legacy paths stay nil-on-group-access.
        if !ctx.group_offsets.is_empty() {
            let span_vals: Vec<Value> = ctx.group_offsets
                .iter()
                .map(|sp| match sp {
                    Some((b, e)) => {
                        let id = self.heap.alloc(HeapObj::Array(
                            vec![Value::Int(*b as i64), Value::Int(*e as i64)].into(),
                        ));
                        Value::Array(id)
                    }
                    None => Value::Nil,
                })
                .collect();
            self.check_alloc()?;
            let off_id = self.heap.alloc(HeapObj::Array(span_vals.into()));
            let off_ivar = self.interner.intern("@group_byte_offsets");
            self.heap.instance_mut(obj_id).ivar_set(off_ivar, Value::Array(off_id));
        }
        if !ctx.cap_names.is_empty() {
            let name_vals: Vec<Value> = ctx.cap_names
                .iter()
                .map(|n| match n {
                    Some(s) => Value::new_str(s.clone()),
                    None => Value::Nil,
                })
                .collect();
            self.check_alloc()?;
            let names_id = self.heap.alloc(HeapObj::Array(name_vals.into()));
            let names_ivar = self.interner.intern("@cap_names");
            self.heap.instance_mut(obj_id).ivar_set(names_ivar, Value::Array(names_id));
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
        let extracted = self.scoped_last_match().map(|lm| {
            // BINARY subject: rebuild positional captures from the raw
            // bytes + spans, tagged ASCII-8BIT, so an invalid byte in a
            // group (e.g. a multipart filename) survives instead of
            // being U+FFFD-mangled by `caps`'s lossy strings. (`@whole` /
            // pre/post stay lossy — the String-typed materialize path —
            // a documented gap; positional `[n]` is what rack reads.)
            let caps: Vec<Value> = if let Some(bc) = &lm.binary {
                bc.group_spans
                    .iter()
                    .map(|span| match span {
                        Some((a, b)) => Value::new_str_bytes_binary(bc.input[*a..*b].to_vec()),
                        None => Value::Nil,
                    })
                    .collect()
            } else {
                lm.caps
                    .iter()
                    .map(|c| match c {
                        Some(s) => Value::new_str(s.clone()),
                        None => Value::Nil,
                    })
                    .collect()
            };
            // Prefer the BINARY per-group spans (byte coords == char
            // indices for ASCII-8BIT) when present; otherwise the
            // UTF-8 `group_spans`. Both are full-`input` coordinates.
            let group_offsets: Vec<Option<(usize, usize)>> = match &lm.binary {
                Some(bc) => bc.group_spans.clone(),
                None => lm.group_spans.clone(),
            };
            let ctx = MatchDataContext {
                pre_match: lm.input.get(..lm.m_start).map(|s| s.to_string()),
                post_match: lm.input.get(lm.m_end..).map(|s| s.to_string()),
                string: Some(lm.input.clone()),
                regexp: None,
                named_captures: lm.named.clone(),
                group_offsets,
                cap_names: lm.cap_names.clone(),
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

    // NOTE: the old `do_regexp_match(re, bound)` pos-0 shorthand was
    // retired in S8 — `Regexp#match` now delegates to
    // `string_match_run` (the same runner `String#match` uses), so
    // every caller reaches `do_regexp_match_pos` / `_at` directly.

    /// `String#match(re, pos)` / `Regexp#match(str, pos)` — match
    /// starting at byte offset `byte_start` within `bound`, with the
    /// FULL string as anchor context (`captures_owned_at`): `\A`/`^`/
    /// `\b`/lookbehind behave like CRuby's onig `pos` — S8 fix, the
    /// old tail-slice made `/^l/.match("hello", 2)` a hit (probed
    /// 3.4.8: nil). Spans come back absolute, so `$~`, `#begin`,
    /// `#pre_match` are relative to the whole subject with no
    /// shifting. `byte_start == 0` is the plain whole-string match.
    pub(crate) fn do_regexp_match_pos(
        &mut self,
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        bound: String,
        byte_start: usize,
    ) -> Result<Value, Trap> {
        // A `\G`-anchored pattern (stripped at compile time) must match
        // EXACTLY at the search position. Use the anchored engine
        // (`\A(?:…)` over the tail slice — its spans are tail-relative,
        // hence span_base = byte_start); fall back to the positioned
        // forward search if no anchored engine could be built.
        let (owned, span_base) = if re.g_anchored()
            && let Some(inner) = re.captures_owned_str_anchored(&bound[byte_start..])
        {
            (inner, byte_start)
        } else {
            (
                re.captures_owned_at(&bound, byte_start)
                    .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?,
                0,
            )
        };
        match owned {
            None => {
                self.save_match_scope_on_write();
                self.last_match = None;
                Ok(Value::Nil)
            }
            Some(oc) => self.finish_regexp_match_hit(re, bound, span_base, oc),
        }
    }

    /// Shared HIT tail for the match-at-pos family: shift spans by
    /// `span_base` (nonzero ONLY for the `\G` anchored-engine path,
    /// whose spans are tail-relative; the positioned search returns
    /// absolute spans and passes 0), set `$~`, and materialize the
    /// MatchData. `bound` is the FULL subject string (ownership
    /// moves into `last_match.input`).
    fn finish_regexp_match_hit(
        &mut self,
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        bound: String,
        span_base: usize,
        mut oc: crate::regex_engine::OwnedCaptures,
    ) -> Result<Value, Trap> {
        // Shift the whole-match span from tail-relative to
        // full-string-relative; group substrings/names are
        // position-independent and need no adjustment.
        oc.m_start += span_base;
        oc.m_end += span_base;
        // Shift group spans from tail-relative to full-string
        // coordinates too (parallel to the whole-match shift).
        let group_spans: Vec<Option<(usize, usize)>> = oc.group_spans
            .iter()
            .map(|sp| sp.map(|(b, e)| (b + span_base, e + span_base)))
            .collect();
        let cap_names = re.capture_group_names();
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
        self.save_match_scope_on_write();
        self.last_match = Some(crate::vm::LastMatch {
            whole: oc.whole.clone(),
            caps: oc.groups.clone(),
            input: bound,
            m_start: oc.m_start,
            m_end: oc.m_end,
            named: oc.named.clone(),
            group_spans: group_spans.clone(),
            cap_names: cap_names.clone(),
            binary: None,
        });
        let ctx = MatchDataContext {
            pre_match: Some(pre),
            post_match: Some(post),
            string: Some(full_str),
            regexp: Some(Value::Regex(re.clone())),
            named_captures: oc.named,
            group_offsets: group_spans,
            cap_names,
        };
        self.materialize_match_data_with_context(oc.whole, group_vals, ctx)
    }

    /// Zero-copy sibling of `do_regexp_match_pos` for a receiver whose
    /// content is KNOWN-valid UTF-8 (`is_utf8_cached`): the engines run
    /// on a borrowed view of the content bytes, so a MISS allocates
    /// NOTHING (the old path copied the whole subject + walked its
    /// chars per call — 13µs on rubocop's 21KB source buffer vs
    /// CRuby's 80ns for `Token#space_after?`). Only a HIT pays the
    /// full-string copy that `$~` / MatchData semantics need.
    ///
    /// SAFETY contract: the caller must have checked
    /// `s.content.is_utf8_cached()`; the unchecked view is sound
    /// because every content mutation goes through `borrow_mut`,
    /// which resets the validity cache.
    pub(crate) fn do_regexp_match_at(
        &mut self,
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        s: &std::rc::Rc<crate::value::RStr>,
        byte_start: usize,
    ) -> Result<Value, Trap> {
        let owned = {
            let bytes = s.content.borrow();
            debug_assert!(std::str::from_utf8(&bytes).is_ok());
            // SAFETY: guarded by `is_utf8_cached` at every call site
            // (see the method doc); `byte_start` is a char-boundary
            // offset produced by the ASCII identity or `char_starts`.
            let view = unsafe { std::str::from_utf8_unchecked(&bytes) };
            // Same `\G` discipline as `do_regexp_match_pos`: anchored
            // engine over the tail (tail-relative spans → span_base =
            // byte_start), positioned full-context search otherwise
            // (absolute spans → span_base = 0).
            if re.g_anchored()
                && let Some(inner) = re.captures_owned_str_anchored(&view[byte_start..])
            {
                Ok((inner, byte_start))
            } else {
                re.captures_owned_at(view, byte_start).map(|o| (o, 0))
            }
        };
        let (owned, span_base) = owned.map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?;
        match owned {
            None => {
                self.save_match_scope_on_write();
                self.last_match = None;
                Ok(Value::Nil)
            }
            Some(oc) => {
                // HIT: `$~`/MatchData snapshot the subject, so the full
                // copy is paid here — and only here.
                let bound = {
                    let bytes = s.content.borrow();
                    // SAFETY: same guard as above.
                    unsafe { std::str::from_utf8_unchecked(&bytes) }.to_string()
                };
                self.finish_regexp_match_hit(re, bound, span_base, oc)
            }
        }
    }

    /// Resolve the optional `pos` argument of the match family with
    /// CRuby's num2long shape (probed 3.4.8): absent → `None`; Int
    /// passes through; Float truncates toward zero via
    /// `float_to_int_arg` (NaN/±Inf → RangeError — CRuby accepts
    /// `match(s, 1.9)`); anything else raises the num2long TypeError
    /// (nil → "no implicit conversion from nil to integer").
    #[cfg(feature = "regex")]
    pub(crate) fn match_pos_arg(&self, pos: Option<&Value>) -> Result<Option<i64>, Trap> {
        match pos {
            None => Ok(None),
            Some(Value::Int(p)) => Ok(Some(*p)),
            Some(Value::Float(f)) => Ok(Some(self.float_to_int_arg(*f)?)),
            Some(other) => Err(self.trap(crate::error::RubyError::TypeError {
                msg: other.num2int_conv_msg(),
            })),
        }
    }

    /// Shared `String#match` runner: validate args (`pattern[, pos]`),
    /// coerce a String pattern to a Regexp, resolve `pos` (char index,
    /// negative counts from the end) to a byte offset, run the match
    /// (binary or UTF-8), set `$~`, and return the MatchData Value (or
    /// Nil). Used by BOTH the plain dispatch arm and the block form
    /// (`str.match(re) { |m| … }`) so they share one source of truth.
    #[cfg(feature = "regex")]
    pub(crate) fn string_match_run(
        &mut self,
        s: &std::rc::Rc<crate::value::RStr>,
        args: &[Value],
    ) -> Result<Value, Trap> {
        use crate::error::RubyError;
        if args.is_empty() || args.len() > 2 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 1..2)",
                    args.len(),
                ),
            }));
        }
        // Coerce a String pattern into a Regex via the same path the
        // `/.../` literal takes (a bad pattern raises RegexpError).
        let coerced: Option<Value> = if let Value::Str(needle) = &args[0] {
            let pat = needle.to_string_lossy();
            let translated = crate::vm::step::preprocess_regex_pattern(&pat);
            let mut compiled = crate::regex_engine::compile(&translated).map_err(|e| {
                self.trap(RubyError::SyntaxError {
                    msg: format!("invalid regex /{}/: {}", pat, e),
                })
            })?;
            compiled.set_g_anchored(crate::vm::step::leading_g_anchor(&pat));
            Some(Value::Regex(std::rc::Rc::new(compiled)))
        } else {
            None
        };
        let regex_arg = coerced.as_ref().unwrap_or(&args[0]);
        let Value::Regex(re) = regex_arg else {
            return Err(self.trap(RubyError::TypeError {
                msg: format!(
                    "wrong argument type {} (expected Regexp)",
                    args[0].type_name(),
                ),
            }));
        };
        let re = re.clone();
        let is_binary = matches!(s.encoding.get(), crate::value::EncodingTag::Binary);
        // Fast path: ASCII / valid-UTF-8 receiver. Resolve `pos` in
        // O(1) (ASCII: char == byte; else the cached char→byte table)
        // and run the match on a BORROWED view of the content — the
        // old path copied the whole subject (`to_string_lossy`) AND
        // walked every char (`chars().count()` + `char_indices().nth`)
        // on EVERY call, which made rubocop's `Token#space_after?`
        // (`source.match(/\G\s/, end_pos)` on a 21KB buffer) 165×
        // CRuby. Invalid-UTF-8 / BINARY receivers keep the paths below.
        if !is_binary && s.content.is_utf8_cached() {
            let byte_start = match self.match_pos_arg(args.get(1))? {
                None => Some(0usize),
                Some(p) => {
                    if s.content.is_ascii_cached() {
                        let char_len = s.content.borrow().len() as i64;
                        let idx = if p < 0 { p + char_len } else { p };
                        if idx < 0 || idx > char_len { None } else { Some(idx as usize) }
                    } else {
                        let starts = s.content.char_starts();
                        let char_len = (starts.len() - 1) as i64;
                        let idx = if p < 0 { p + char_len } else { p };
                        if idx < 0 || idx > char_len {
                            None
                        } else {
                            Some(starts[idx as usize] as usize)
                        }
                    }
                }
            };
            return match byte_start {
                // Out-of-range pos → no match (nil), matching CRuby.
                None => {
                    self.save_match_scope_on_write();
                    self.last_match = None;
                    Ok(Value::Nil)
                }
                Some(b) => self.do_regexp_match_at(&re, s, b),
            };
        }
        let bound = s.to_string_lossy();
        let char_len = bound.chars().count();
        // Resolve the optional char-index `pos`. Negative counts from
        // the end; out-of-range → no match (nil), matching CRuby.
        let byte_start = match self.match_pos_arg(args.get(1))? {
            None => 0,
            Some(p) => {
                let idx = if p < 0 { p + char_len as i64 } else { p };
                if idx < 0 || idx > char_len as i64 {
                    self.save_match_scope_on_write();
                    self.last_match = None;
                    return Ok(Value::Nil);
                }
                bound.char_indices().nth(idx as usize).map(|(b, _)| b).unwrap_or(bound.len())
            }
        };
        // BINARY subject (only at pos 0): byte engine + byte-faithful
        // captures. A non-zero pos falls through to the lossy UTF-8
        // path (binary+pos is vanishingly rare; the existing @whole /
        // pre / post are lossy there anyway).
        if byte_start == 0
            && is_binary
            && let Some(v) = self.do_regexp_match_binary(&re, s)?
        {
            return Ok(v);
        }
        self.do_regexp_match_pos(&re, bound, byte_start)
    }

    /// `String#match` / `Regexp#match` against an ASCII-8BIT (BINARY)
    /// subject — runs the byte engine so the match works at all (a
    /// lossy UTF-8 `bound` both breaks byte-level patterns like
    /// `/\xC3/n` AND U+FFFD-mangles captures) and the captures come
    /// back byte-faithful. Returns `Ok(None)` when there's no byte
    /// engine (Unicode-needing / fancy pattern), so the caller falls
    /// back to the lossy `do_regexp_match`. Sets `$~` (with the binary
    /// span data) and reuses `materialize_last_match`, which already
    /// rebuilds positional captures from the raw bytes.
    pub(crate) fn do_regexp_match_binary(
        &mut self,
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        s: &std::rc::Rc<crate::value::RStr>,
    ) -> Result<Option<Value>, Trap> {
        let (owned, input): (Option<crate::regex_engine::OwnedCaptures>, Vec<u8>) = {
            let bytes = s.content.borrow();
            match re.captures_owned_bytes(&bytes) {
                Some(o) => (o, bytes.to_vec()),
                None => return Ok(None),
            }
        };
        self.save_match_scope_on_write();
        match owned {
            None => {
                self.last_match = None;
                Ok(Some(Value::Nil))
            }
            Some(oc) => {
                let input_lossy = String::from_utf8_lossy(&input).into_owned();
                // `binary.group_spans` (full-input byte coords) backs the
                // offset accessors for a BINARY subject — materialize
                // prefers it — so the UTF-8 `group_spans` slot stays empty.
                let cap_names = re.capture_group_names();
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole,
                    caps: oc.groups,
                    input: input_lossy,
                    m_start: oc.m_start,
                    m_end: oc.m_end,
                    named: oc.named,
                    group_spans: Vec::new(),
                    cap_names,
                    binary: Some(crate::vm::BinaryCaps {
                        input: input.into_boxed_slice(),
                        group_spans: oc.group_spans,
                    }),
                });
                // materialize_last_match reads `binary` → byte-faithful
                // positional captures, ASCII-8BIT tagged.
                Ok(Some(self.materialize_last_match()?))
            }
        }
    }

    /// StringScanner search over a BINARY buffer, starting at byte
    /// offset `start`, WITHOUT copying the tail. The Ruby idiom
    /// `@str[@pos..] =~ re` allocates an O(remaining) String on EVERY
    /// scan, turning a multi-part `scan_until` loop into O(n²) (rack
    /// multipart's 10 000-part body). Here we take a `&bytes[start..]`
    /// SUBSLICE — an O(1) view in Rust — which ALSO gives the correct
    /// StringScanner anchoring: `\A` / `^` anchor at the scan position
    /// (the subslice start), exactly as CRuby's StringScanner does
    /// (verified: `\A` matches at `@pos`, not byte 0). The byte engine
    /// runs on the view; offsets are shifted back to absolute. Sets
    /// `$~` (with binary span data so `$~[1]` etc. stay byte-faithful)
    /// and returns:
    /// * `Int(abs_start)` — match found; `$~` is set;
    /// * `Nil` — no match at/after `start`; `$~` cleared;
    /// * `Bool(false)` — no byte engine for this pattern, so the caller
    ///   must fall back to the slice path.
    pub(crate) fn do_strscan_search_binary(
        &mut self,
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        s: &std::rc::Rc<crate::value::RStr>,
        start: usize,
    ) -> Result<Value, Trap> {
        let start = start.min(s.content.borrow().len());
        // Returns: None ⇒ no byte engine; Some(None) ⇒ no match;
        // Some(Some((abs_start, region_bytes, group_spans_rel, oc))).
        // CRITICAL: copy only the MATCHED REGION (O(match-len)), never
        // the whole buffer — a per-scan `to_vec()` of the full string
        // would reintroduce the O(n²) we just removed.
        type Hit = (usize, Vec<u8>, Vec<Option<(usize, usize)>>, crate::regex_engine::OwnedCaptures);
        let extracted: Option<Option<Hit>> = {
            let bytes = s.content.borrow();
            // O(1) view, NOT a copy. `\A`/`^` anchor at `start`.
            let sub = &bytes[start..];
            match re.captures_owned_bytes(sub) {
                None => None,
                Some(None) => Some(None),
                Some(Some(oc)) => {
                    let region = sub[oc.m_start..oc.m_end].to_vec();
                    // Re-base group spans onto the region (groups are
                    // always within the overall match → no underflow).
                    let base = oc.m_start;
                    let spans = oc
                        .group_spans
                        .iter()
                        .map(|sp| sp.map(|(a, b)| (a - base, b - base)))
                        .collect();
                    Some(Some((oc.m_start + start, region, spans, oc)))
                }
            }
        };
        self.save_match_scope_on_write();
        match extracted {
            None => Ok(Value::Bool(false)),
            Some(None) => {
                self.last_match = None;
                Ok(Value::Nil)
            }
            Some(Some((abs_start, region, group_spans, oc))) => {
                // `$~` is stored relative to the matched region (m_start
                // = 0). StringScanner computes pre/post-match from its
                // own `@str`/`@match_pos`, so it never reads `$~`'s
                // absolute span; `$~[0]`/`$~[n]` stay byte-faithful via
                // the region bytes. The ABSOLUTE match start is the
                // return value (the scanner's `@match_pos`).
                let match_len = region.len();
                let input_lossy = String::from_utf8_lossy(&region).into_owned();
                let cap_names = re.capture_group_names();
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole,
                    caps: oc.groups,
                    input: input_lossy,
                    m_start: 0,
                    m_end: match_len,
                    named: oc.named,
                    group_spans: Vec::new(),
                    cap_names,
                    binary: Some(crate::vm::BinaryCaps {
                        input: region.into_boxed_slice(),
                        group_spans,
                    }),
                });
                Ok(Value::Int(abs_start as i64))
            }
        }
    }

    /// Anchored sibling of `do_strscan_search_binary` backing
    /// `StringScanner#scan`/`check`/`skip`/`match?` (the `match_at_pos`
    /// path). The match must BEGIN exactly at `start` (the scanner's
    /// `@pos`); a match found further ahead is rejected. The win over
    /// the Ruby `slice = @str[@pos..]; slice =~ regex` shape is that the
    /// tail is a zero-copy `&bytes[start..]` view, not a per-call
    /// O(remaining) copy — that copy is what made kramdown's
    /// scan-at-every-position loop O(n²). Returns: `false` ⇒ no byte
    /// engine (scanner falls back to the slice path); `nil` ⇒ no
    /// anchored match; `Str(matched)` ⇒ the matched substring (with `$~`
    /// also set so the scanner's `[]`/`matched`/captures still work). The
    /// scanner advances `@pos` by `matched.length` — returning the string
    /// rather than a length lets `scan`/`check` skip a `$~[0]` round-trip.
    pub(crate) fn do_strscan_match_at_binary(
        &mut self,
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        s: &std::rc::Rc<crate::value::RStr>,
        start: usize,
    ) -> Result<Value, Trap> {
        let start = start.min(s.content.borrow().len());
        // `Bytes`: a linear-engine hit (byte-faithful captures).
        // `Str`: a fancy-engine hit (the pattern has no byte engine);
        // valid because `@byte_addressable` ⇒ the view is ASCII, so
        // byte offset == char offset and the captures are valid UTF-8.
        enum Outcome {
            NoEngine,
            NoMatch,
            Bytes(Vec<u8>, Vec<Option<(usize, usize)>>, crate::regex_engine::OwnedCaptures),
            Str(crate::regex_engine::OwnedCaptures),
        }
        let outcome = {
            let bytes = s.content.borrow();
            // O(1) view, NOT a copy. The anchored engines (`\A(?:…)`)
            // force the match to begin at the view start, so a miss
            // fails fast instead of forward-scanning the tail.
            let sub = &bytes[start..];
            match re.captures_owned_bytes_anchored(sub) {
                Some(None) => Outcome::NoMatch,
                Some(Some(oc)) => {
                    // m_start is 0 by construction (the `\A` anchor).
                    let region = sub[oc.m_start..oc.m_end].to_vec();
                    let base = oc.m_start;
                    let spans = oc
                        .group_spans
                        .iter()
                        .map(|sp| sp.map(|(a, b)| (a - base, b - base)))
                        .collect();
                    Outcome::Bytes(region, spans, oc)
                }
                // No linear byte engine (lookaround / backref): try the
                // anchored FANCY engine over the ASCII view. This is the
                // path kramdown's block-boundary `check`s take.
                None => match std::str::from_utf8(sub) {
                    Err(_) => Outcome::NoEngine,
                    Ok(sub_str) => match re.captures_owned_str_anchored(sub_str) {
                        None => Outcome::NoEngine,
                        Some(None) => Outcome::NoMatch,
                        Some(Some(oc)) => Outcome::Str(oc),
                    },
                },
            }
        };
        self.save_match_scope_on_write();
        match outcome {
            Outcome::NoEngine => Ok(Value::Bool(false)),
            Outcome::NoMatch => {
                self.last_match = None;
                Ok(Value::Nil)
            }
            Outcome::Bytes(region, group_spans, oc) => {
                let match_len = region.len();
                let matched = String::from_utf8_lossy(&region).into_owned();
                let cap_names = re.capture_group_names();
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole,
                    caps: oc.groups,
                    input: matched.clone(),
                    m_start: 0,
                    m_end: match_len,
                    named: oc.named,
                    group_spans: Vec::new(),
                    cap_names,
                    binary: Some(crate::vm::BinaryCaps {
                        input: region.into_boxed_slice(),
                        group_spans,
                    }),
                });
                // Return the matched substring directly (not the length):
                // the scanner advances `@pos` by its `.length` and uses it
                // as `scan`/`check`'s result, so it needn't re-read `$~[0]`.
                Ok(Value::new_str(matched))
            }
            Outcome::Str(oc) => {
                // ASCII view ⇒ byte len == char len; the matched span
                // begins at 0 (`\A`), so `$~`'s region is `oc.whole`.
                let match_len = oc.whole.len();
                let cap_names = re.capture_group_names();
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole.clone(),
                    caps: oc.groups,
                    input: oc.whole.clone(),
                    m_start: 0,
                    m_end: match_len,
                    named: oc.named,
                    group_spans: oc.group_spans,
                    cap_names,
                    binary: None,
                });
                Ok(Value::new_str(oc.whole))
            }
        }
    }
}
