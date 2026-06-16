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
                self.save_match_scope_on_write();
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
                self.save_match_scope_on_write();
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole.clone(),
                    caps: oc.groups.clone(),
                    input: bound,
                    m_start: oc.m_start,
                    m_end: oc.m_end,
                    named: oc.named.clone(),
                    binary: None,
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
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole,
                    caps: oc.groups,
                    input: input_lossy,
                    m_start: oc.m_start,
                    m_end: oc.m_end,
                    named: oc.named,
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
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole,
                    caps: oc.groups,
                    input: input_lossy,
                    m_start: 0,
                    m_end: match_len,
                    named: oc.named,
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
    /// anchored match; `Int(len)` ⇒ matched byte/char length (with
    /// `@byte_addressable` true, byte == char), with `$~` set so the
    /// caller's `$~[0]` yields the matched substring.
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
                let input_lossy = String::from_utf8_lossy(&region).into_owned();
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole,
                    caps: oc.groups,
                    input: input_lossy,
                    m_start: 0,
                    m_end: match_len,
                    named: oc.named,
                    binary: Some(crate::vm::BinaryCaps {
                        input: region.into_boxed_slice(),
                        group_spans,
                    }),
                });
                Ok(Value::Int(match_len as i64))
            }
            Outcome::Str(oc) => {
                // ASCII view ⇒ byte len == char len; the matched span
                // begins at 0 (`\A`), so `$~`'s region is `oc.whole`.
                let match_len = oc.whole.len();
                self.last_match = Some(crate::vm::LastMatch {
                    whole: oc.whole.clone(),
                    caps: oc.groups,
                    input: oc.whole,
                    m_start: 0,
                    m_end: match_len,
                    named: oc.named,
                    binary: None,
                });
                Ok(Value::Int(match_len as i64))
            }
        }
    }
}
