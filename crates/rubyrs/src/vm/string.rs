//! `String` primitive methods. Mirrors CRuby's `string.c` —
//! the per-method match arms that don't need heap allocation
//! (concat / sub / gsub / tr already produce String results via
//! `Value::new_str`, which wraps a fresh Rc<RStr>; nothing here
//! reaches into the GC heap directly).
//!
//! Called from `primitive_call` (vm.rs) after numeric dispatch.
//! Stateless — no Vm access, just receiver + args + the
//! resource cap.

use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{Instance, RStr, Value};

use super::{ruby_sprintf, Vm};

/// Try the Str primitive arms. Returns `Ok(Some(v))` on a
/// handled call, `Ok(None)` if the receiver/method shape
/// doesn't match.
pub(crate) fn string_call(
    recv: &Value,
    name: &str,
    args: &[Value],
    max_value_bytes: Option<usize>,
) -> Result<Option<Value>, RubyError> {
    // Helper: enforce the per-value byte cap at every
    // string-growing arm. Returns Err if the projected size
    // would exceed the cap; callers wrap it in `Trap`.
    let check = |new_len: usize| -> Result<(), RubyError> {
        if let Some(max) = max_value_bytes
            && new_len > max {
                return Err(RubyError::ResourceExhausted {
                    msg: format!("value size {new_len} bytes > cap {max}"),
                });
            }
        Ok(())
    };
    Ok(match (recv, name, args) {
        (Value::Str(a), "+", [Value::Str(b)]) => {
            check(a.borrow().len().saturating_add(b.borrow().len()))?;
            let mut s = a.borrow().clone();
            s.push_str(&b.borrow());
            Some(Value::new_str(s))
        }
        (Value::Str(a), "==", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() == *b.borrow())),
        (Value::Str(a), "!=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() != *b.borrow())),
        (Value::Str(a), "to_s", []) => Some(Value::Str(a.clone())),
        (Value::Str(a), "length", []) | (Value::Str(a), "size", []) => Some(Value::Int(a.borrow().chars().count() as i64)),
        (Value::Str(a), "empty?", []) => Some(Value::Bool(a.borrow().is_empty())),
        (Value::Str(a), "upcase", []) => Some(Value::new_str(a.borrow().to_uppercase())),
        (Value::Str(a), "downcase", []) => Some(Value::new_str(a.borrow().to_lowercase())),
        (Value::Str(a), "reverse", []) => Some(Value::new_str(a.borrow().chars().rev().collect::<String>())),
        // `String#succ` / `#next` — Ruby's "alphanumeric successor".
        // We support the common single-letter case (`'a'.succ == 'b'`,
        // `'Z'.succ == 'AA'`) plus the general "rightmost alnum
        // rolls over with carry" rule via `str_succ`. The pure-
        // digit / non-alnum and bracketed-string edge cases are
        // documented gaps; CRuby diff fixtures pin the supported
        // shape.
        (Value::Str(a), "succ", []) | (Value::Str(a), "next", []) => {
            Some(Value::new_str(str_succ(&a.borrow())))
        }
        (Value::Str(a), "strip", []) => Some(Value::new_str(a.borrow().trim().to_string())),
        (Value::Str(a), "lstrip", []) => Some(Value::new_str(a.borrow().trim_start().to_string())),
        (Value::Str(a), "rstrip", []) => Some(Value::new_str(a.borrow().trim_end().to_string())),
        (Value::Str(a), "include?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().contains(&*b.borrow()))),
        // Literal-substring `match?` — true iff the receiver
        // contains the argument as a substring. CRuby additionally
        // accepts a Regexp here; we only handle String, in line
        // with the rest of our regex-free subset. Calls with a
        // non-String argument fall through to NoMethodError.
        (Value::Str(a), "match?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().contains(&*b.borrow()))),
        // String#match? with a Regex — proper regex match. Returns
        // bool without populating any match-data side state.
        (Value::Str(a), "match?", [Value::Regex(re)]) => {
            Some(Value::Bool(re.is_match(&a.borrow())))
        }
        // `index(substr)` / `rindex(substr)` — return the byte
        // offset where the substring first / last appears, or
        // nil if it's absent. CRuby reports a *character* index
        // for non-ASCII receivers; we report `String::find`'s
        // byte index, which matches for ASCII (the common case
        // for our test fixtures) and diverges for multibyte —
        // documented in SUBSET.md.
        (Value::Str(a), "index", [Value::Str(b)]) => {
            Some(match a.borrow().find(&*b.borrow()) {
                Some(i) => Value::Int(i as i64),
                None => Value::Nil,
            })
        }
        (Value::Str(a), "rindex", [Value::Str(b)]) => {
            Some(match a.borrow().rfind(&*b.borrow()) {
                Some(i) => Value::Int(i as i64),
                None => Value::Nil,
            })
        }
        // Literal-substring sub/gsub. Regex forms (`gsub(/pat/, ...)`)
        // are out of scope until we add a regex engine — documented
        // in SUBSET.md. CRuby's `gsub("", "x")` on a non-empty
        // string inserts at every character boundary; we replicate
        // that via `Rust`'s `str::replace` for non-empty patterns
        // and a hand-rolled walk for the empty-pattern case.
        (Value::Str(a), "sub", [Value::Str(pat), Value::Str(repl)]) => {
            let a_ref = a.borrow();
            let pat_ref = pat.borrow();
            let repl_ref = repl.borrow();
            let out = if pat_ref.is_empty() {
                // CRuby: sub("", repl) inserts `repl` at index 0.
                let mut s = repl_ref.clone();
                s.push_str(&a_ref);
                s
            } else if let Some(idx) = a_ref.find(&*pat_ref) {
                let mut s = String::with_capacity(a_ref.len() + repl_ref.len());
                s.push_str(&a_ref[..idx]);
                s.push_str(&repl_ref);
                s.push_str(&a_ref[idx + pat_ref.len()..]);
                s
            } else {
                a_ref.clone()
            };
            check(out.len())?;
            Some(Value::new_str(out))
        }
        // Regex form: `s.sub(/pat/, "repl")`. Replacement string
        // supports Ruby backrefs `\0` / `\1` / ... — translate to
        // the `regex` crate's `$0` / `$1` syntax. `\\` escapes a
        // literal backslash. Block form
        // (`s.sub(/pat/) { |m| ... }`) is the higher-value but
        // separately-dispatched path; not handled here.
        (Value::Str(a), "sub", [Value::Regex(re), Value::Str(repl)]) => {
            let a_ref = a.borrow();
            let repl_ref = repl.borrow();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            let out = re.replace(&a_ref, repl_xlated.as_str()).into_owned();
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "gsub", [Value::Regex(re), Value::Str(repl)]) => {
            let a_ref = a.borrow();
            let repl_ref = repl.borrow();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            let out = re.replace_all(&a_ref, repl_xlated.as_str()).into_owned();
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "gsub", [Value::Str(pat), Value::Str(repl)]) => {
            let a_ref = a.borrow();
            let pat_ref = pat.borrow();
            let repl_ref = repl.borrow();
            let out = if pat_ref.is_empty() {
                // CRuby: gsub("", repl) wraps `repl` around every
                // character — `"abc".gsub("", "X") == "XaXbXcX"`.
                let mut s = repl_ref.clone();
                for c in a_ref.chars() {
                    s.push(c);
                    s.push_str(&repl_ref);
                }
                s
            } else {
                a_ref.replace(&*pat_ref, &repl_ref)
            };
            check(out.len())?;
            Some(Value::new_str(out))
        }
        // String#tr — character-by-character translation. Each
        // char in `from` maps to the same-index char in `to`; if
        // `to` is shorter, characters past its length map to its
        // LAST char (CRuby's "stretch" behaviour). If `to` is
        // empty, those chars are deleted. Character-range syntax
        // (`"a-z"`) is intentionally NOT expanded — flagged in
        // SUBSET.md.
        (Value::Str(a), "tr", [Value::Str(from), Value::Str(to)]) => {
            let a_ref = a.borrow();
            let from_ref = from.borrow();
            let to_ref = to.borrow();
            let from_chars: Vec<char> = from_ref.chars().collect();
            let to_chars: Vec<char> = to_ref.chars().collect();
            let mut out = String::with_capacity(a_ref.len());
            for ch in a_ref.chars() {
                if let Some(idx) = from_chars.iter().position(|c| *c == ch) {
                    if to_chars.is_empty() {
                        // Delete: skip this character entirely.
                    } else if idx < to_chars.len() {
                        out.push(to_chars[idx]);
                    } else {
                        out.push(*to_chars.last().unwrap());
                    }
                } else {
                    out.push(ch);
                }
            }
            check(out.len())?;
            Some(Value::new_str(out))
        }
        // `String#squeeze` — collapse consecutive runs of the same
        // character. With a char-set arg, only chars in the set
        // are squeezed. Char-set ranges (`"a-z"`) and ^-negation
        // are NOT expanded here — same conservative semantics as
        // `tr`. Documented in SUBSET.md.
        (Value::Str(a), "squeeze", rest) if rest.is_empty()
            || (rest.len() == 1 && matches!(rest[0], Value::Str(_))) => {
            let a_ref = a.borrow();
            let set: Option<Vec<char>> = match rest.first() {
                None => None,
                Some(Value::Str(s)) => Some(s.borrow().chars().collect()),
                _ => unreachable!(),
            };
            let mut out = String::with_capacity(a_ref.len());
            let mut prev: Option<char> = None;
            for ch in a_ref.chars() {
                let in_set = match &set {
                    Some(s) => s.iter().any(|c| *c == ch),
                    None => true,
                };
                if in_set && Some(ch) == prev {
                    continue;
                }
                out.push(ch);
                prev = Some(ch);
            }
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "start_with?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().starts_with(&*b.borrow()))),
        (Value::Str(a), "end_with?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().ends_with(&*b.borrow()))),
        (Value::Str(a), "to_i", []) => {
            // CRuby's `String#to_i` is famously lenient: leading
            // whitespace, optional sign, then as many digits as it
            // can read; non-numeric tail (or empty input) gives 0.
            let a_ref = a.borrow();
            let s = a_ref.trim_start();
            let (sign, rest) = match s.as_bytes().first() {
                Some(b'-') => (-1i64, &s[1..]),
                Some(b'+') => (1i64, &s[1..]),
                _ => (1i64, s),
            };
            let mut n: i64 = 0;
            let mut saw_digit = false;
            for c in rest.chars() {
                if let Some(d) = c.to_digit(10) {
                    saw_digit = true;
                    n = n.wrapping_mul(10).wrapping_add(d as i64);
                } else { break; }
            }
            Some(Value::Int(if saw_digit { sign.wrapping_mul(n) } else { 0 }))
        }
        (Value::Str(a), "to_f", []) => {
            // CRuby's leniency: trim leading whitespace, parse what
            // we can, return 0.0 for "garbage". Rust's stdlib
            // `f64::from_str` is stricter (rejects trailing junk),
            // so we scan a Ruby-shaped prefix ourselves.
            let a_ref = a.borrow();
            let s = a_ref.trim_start();
            let bytes = s.as_bytes();
            let mut end = 0usize;
            if bytes.first() == Some(&b'-') || bytes.first() == Some(&b'+') {
                end += 1;
            }
            let mut saw_digit = false;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                saw_digit = true;
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'.' {
                end += 1;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    saw_digit = true;
                    end += 1;
                }
            }
            // Optional exponent
            if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
                let mut e = end + 1;
                if e < bytes.len() && (bytes[e] == b'+' || bytes[e] == b'-') { e += 1; }
                let exp_start = e;
                while e < bytes.len() && bytes[e].is_ascii_digit() { e += 1; }
                if e > exp_start { end = e; }
            }
            let parsed = if saw_digit {
                s[..end].parse::<f64>().unwrap_or(0.0)
            } else { 0.0 };
            Some(Value::Float(parsed))
        }
        (Value::Str(a), "*", [Value::Int(n)]) => {
            let n = (*n).max(0) as usize;
            check(a.borrow().len().saturating_mul(n))?;
            Some(Value::new_str(a.borrow().repeat(n)))
        }
        (Value::Str(a), "<", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() < *b.borrow())),
        (Value::Str(a), "<=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() <= *b.borrow())),
        (Value::Str(a), ">", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() > *b.borrow())),
        (Value::Str(a), "<=>", [Value::Str(b)]) => Some(Value::Int(a.borrow().cmp(&*b.borrow()) as i64)),
        (Value::Str(a), ">=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() >= *b.borrow())),
        // Regex#match? mirror — same semantics either side.
        (Value::Regex(re), "match?", [Value::Str(s)]) => {
            Some(Value::Bool(re.is_match(&s.borrow())))
        }
        // Regex#source — the raw pattern string.
        (Value::Regex(re), "source", []) => Some(Value::new_str(re.as_str().to_string())),
        (Value::Regex(re), "to_s", []) => Some(Value::new_str(format!("(?-mix:{})", re.as_str()))),
        (Value::Regex(re), "inspect", []) => Some(Value::new_str(format!("/{}/", re.as_str()))),
        // String#inspect — wrap in double quotes, escape `\`,
        // `"`, and common control characters. Matches CRuby for
        // printable ASCII + the standard escape set; exotic
        // Unicode escapes (`\u{...}`) are out of scope.
        (Value::Str(s), "inspect", []) => {
            let raw = s.borrow();
            let mut out = String::with_capacity(raw.len() + 2);
            out.push('"');
            for c in raw.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"'  => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\0' => out.push_str("\\0"),
                    _ => out.push(c),
                }
            }
            out.push('"');
            Some(Value::new_str(out))
        }
        _ => None,
    })
}

impl Vm {
    /// String methods that need heap access — slice, scan, []=,
    /// %, freeze / frozen? / dup, and all the in-place
    /// mutators. Mirrors the heap-aware half of CRuby's
    /// `string.c`; the rest lives in `string_call` above.
    /// Dispatched from `Vm::collection_call`'s `Value::Str` arm.
    pub(crate) fn string_collection_call(
        &mut self,
        s: Rc<RStr>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        Ok({
                let s = s.clone();
                // In-place mutation methods. All return the
                // receiver (same Rc, so aliases observe the
                // change). The variadic shape (`concat`, `prepend`
                // take *args) doesn't fit the inner-match
                // `[Value::Str(b)]` pattern; we dispatch by name
                // first, then validate the args.
                // freeze / frozen? / dup — the per-string immutability
                // controls. CRuby raises FrozenError on any mutating
                // method against a frozen string; we route that
                // through a Trap so `rescue FrozenError` catches it.
                if name == "frozen?" && args.is_empty() {
                    return Ok(Some(Value::Bool(s.frozen.get())));
                }
                if name == "freeze" && args.is_empty() {
                    s.frozen.set(true);
                    return Ok(Some(Value::Str(s)));
                }
                if name == "dup" && args.is_empty() {
                    // Fresh Rc, fresh RefCell, NOT frozen — `dup`
                    // copies content but resets the frozen bit.
                    let copy = s.content.borrow().clone();
                    return Ok(Some(Value::new_str(copy)));
                }
                // Helper closure: bail out of any mutating method
                // if `s` was frozen. Used by `<<`, `concat`,
                // `prepend`, `replace`, `[]=`.
                let check_unfrozen = |vm: &Vm| -> Result<(), Trap> {
                    if s.frozen.get() {
                        Err(vm.trap(RubyError::FrozenError {
                            msg: format!("can't modify frozen String: {:?}", s.content.borrow()),
                        }))
                    } else {
                        Ok(())
                    }
                };
                if name == "<<" && args.len() == 1 {
                    check_unfrozen(self)?;
                    match &args[0] {
                        Value::Str(other) => {
                            let to_push = other.borrow().clone();
                            s.borrow_mut().push_str(&to_push);
                        }
                        // CRuby's String#<< also accepts Integer
                        // (treated as a codepoint). Support it
                        // since Rake / Sinatra builders rely on it
                        // for fast char-by-char concatenation.
                        Value::Int(n) => {
                            if let Some(c) = char::from_u32(*n as u32) {
                                s.borrow_mut().push(c);
                            } else {
                                return Err(self.trap(RubyError::ArgumentError {
                                    msg: format!("{} out of char range", n),
                                }));
                            }
                        }
                        other => return Err(self.trap(RubyError::TypeError {
                            msg: format!("no implicit conversion of {} into String", other.type_name()),
                        })),
                    }
                    return Ok(Some(Value::Str(s)));
                }
                if name == "concat" {
                    check_unfrozen(self)?;
                    for a in args {
                        match a {
                            Value::Str(o) => {
                                let to_push = o.borrow().clone();
                                s.borrow_mut().push_str(&to_push);
                            }
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into String", a.type_name()),
                            })),
                        }
                    }
                    return Ok(Some(Value::Str(s)));
                }
                if name == "prepend" {
                    check_unfrozen(self)?;
                    // Concatenate args in order, then prepend to
                    // existing content. CRuby's `prepend("a","b")`
                    // results in `"a" + "b" + self`, not the
                    // reverse — verified against MRI.
                    let mut prefix = String::new();
                    for a in args {
                        match a {
                            Value::Str(o) => prefix.push_str(&o.borrow()),
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into String", a.type_name()),
                            })),
                        }
                    }
                    let mut buf = prefix;
                    buf.push_str(&s.borrow());
                    *s.borrow_mut() = buf;
                    return Ok(Some(Value::Str(s)));
                }
                if name == "replace" && args.len() == 1 {
                    check_unfrozen(self)?;
                    match &args[0] {
                        Value::Str(o) => {
                            let new_content = o.borrow().clone();
                            *s.borrow_mut() = new_content;
                        }
                        other => return Err(self.trap(RubyError::TypeError {
                            msg: format!("no implicit conversion of {} into String", other.type_name()),
                        })),
                    }
                    return Ok(Some(Value::Str(s)));
                }
                // String#[] / #slice — char-indexed slicing.
                // CRuby's semantics:
                //   s[i]           -> single-char String, or nil
                //   s[i, n]        -> substring of n chars from i,
                //                     or nil if i out of bounds
                //                     (i == len is OK and gives "")
                //   s[Range]       -> substring; nil for invalid start
                // Negative indices count from the end; out-of-bounds
                // returns nil. Multibyte strings are sliced by char,
                // not by byte.
                fn str_index_char(chars: &[char], i: i64) -> Option<usize> {
                    let len = chars.len() as i64;
                    let idx = if i < 0 { len + i } else { i };
                    if idx < 0 || idx > len { None }
                    else { Some(idx as usize) }
                }
                fn str_slice(chars: &[char], start: usize, n: usize) -> String {
                    chars.iter().skip(start).take(n).collect()
                }
                // String#match(regex) — returns a MatchData
                // instance with @whole = whole match and
                // @caps = numbered captures (Strings, or nil
                // for groups that didn't participate). Returns
                // nil if no match. CRuby additionally accepts
                // a String (interpreted as a literal regex) and
                // a starting offset; both out of scope here.
                if name == "match" && args.len() == 1 {
                    if let Value::Regex(re) = &args[0] {
                        let bound = s.content.borrow().clone();
                        let captures = re.captures(&bound);
                        match captures {
                            None => return Ok(Some(Value::Nil)),
                            Some(caps) => {
                                let whole = caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default();
                                let mut group_vals: Vec<Value> = Vec::with_capacity(caps.len().saturating_sub(1));
                                for i in 1..caps.len() {
                                    group_vals.push(match caps.get(i) {
                                        Some(m) => Value::new_str(m.as_str().to_string()),
                                        None => Value::Nil,
                                    });
                                }
                                self.maybe_gc();
                                let caps_arr = self.heap.alloc(HeapObj::Array(group_vals));
                                let cls_id = self.interner.intern("MatchData");
                                let cls = match self.classes.get(&cls_id).cloned() {
                                    Some(c) => c,
                                    None => return Ok(Some(Value::Nil)),
                                };
                                let obj_id = self.heap.alloc(HeapObj::Instance(Instance {
                                    class: cls,
                                    ivars: HashMap::new(),
                                    singleton_class: None,
                                }));
                                let whole_ivar = self.interner.intern("@whole");
                                let caps_ivar = self.interner.intern("@caps");
                                {
                                    let inst = self.heap.instance_mut(obj_id);
                                    inst.ivars.insert(whole_ivar, Value::new_str(whole));
                                    inst.ivars.insert(caps_ivar, Value::Array(caps_arr));
                                }
                                return Ok(Some(Value::Object(obj_id)));
                            }
                        }
                    }
                    return Ok(None);
                }
                if (name == "[]" || name == "slice") && args.len() == 1 {
                    let chars: Vec<char> = s.borrow().chars().collect();
                    let len = chars.len() as i64;
                    return Ok(Some(match &args[0] {
                        Value::Int(i) => {
                            let idx = if *i < 0 { len + *i } else { *i };
                            if idx < 0 || idx >= len {
                                Value::Nil
                            } else {
                                let ch = chars[idx as usize].to_string();
                                Value::new_str(ch)
                            }
                        }
                        Value::Range(rid) => {
                            // Endless / beginless: a Nil endpoint
                            // means "from index 0" or "to len". So
                            // (`s[6..]` / `s[..5]` / `s[..]` all
                            // resolve via this branch.
                            let r = self.heap.range(*rid);
                            let excl = r.exclusive;
                            let bi: i64 = match &r.begin {
                                Value::Int(a) => *a,
                                Value::Nil => 0,
                                _ => return Ok(None),
                            };
                            let ei: i64 = match &r.end {
                                Value::Int(c) => *c,
                                Value::Nil => len, // exclusive of len-1 below
                                _ => return Ok(None),
                            };
                            let endless_end = matches!(&r.end, Value::Nil);
                            let start = match str_index_char(&chars, bi) {
                                Some(s) => s,
                                None => return Ok(Some(Value::Nil)),
                            };
                            // End index: positive raw; negative
                            // relative to len. Out-of-range high
                            // clamps to len; exclusive drops one.
                            // Nil end is always "to len" (no
                            // exclusive adjustment).
                            let mut end = if endless_end { len } else if ei < 0 { len + ei } else { ei };
                            if !excl && !endless_end { end += 1; }
                            let end = end.clamp(start as i64, len) as usize;
                            let slice: String = str_slice(&chars, start, end.saturating_sub(start));
                            Value::new_str(slice)
                        }
                        _ => return Ok(None),
                    }));
                }
                if (name == "[]" || name == "slice") && args.len() == 2 {
                    if let (Value::Int(i), Value::Int(n)) = (&args[0], &args[1]) {
                        let chars: Vec<char> = s.borrow().chars().collect();
                        let len = chars.len() as i64;
                        let start_raw = if *i < 0 { len + *i } else { *i };
                        if start_raw < 0 || start_raw > len || *n < 0 {
                            return Ok(Some(Value::Nil));
                        }
                        let start = start_raw as usize;
                        let n = (*n as usize).min(chars.len() - start);
                        let slice = str_slice(&chars, start, n);
                        return Ok(Some(Value::new_str(slice)));
                    }
                    return Ok(None);
                }
                // String#[]= — in-place mutation. Three shapes:
                //   s[i]      = x   → replace one char at char-index i
                //   s[i, n]   = x   → replace n chars from char-index i
                //   s[range]  = x   → replace the slice covered by the range
                // Negative indices count from the end. Out-of-range
                // raises IndexError, matching CRuby (we surface that
                // through the Trap-to-rescue path).
                //
                // The mutation works because Value::Str holds an
                // Rc<RefCell<String>>: every clone of this Value
                // shares the same RefCell, so writes through
                // `borrow_mut` are visible to all aliases.
                if name == "[]=" && args.len() == 2 {
                    check_unfrozen(self)?;
                    if let (Value::Int(i), Value::Str(repl)) = (&args[0], &args[1]) {
                        let chars: Vec<char> = s.borrow().chars().collect();
                        let len = chars.len() as i64;
                        let idx = if *i < 0 { len + *i } else { *i };
                        if idx < 0 || idx >= len {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("index {i} out of string"),
                            }));
                        }
                        let mut buf: String = chars[..idx as usize].iter().collect();
                        buf.push_str(&repl.borrow());
                        buf.extend(chars[idx as usize + 1..].iter());
                        *s.borrow_mut() = buf;
                        return Ok(Some(args[1].clone()));
                    }
                    return Ok(None);
                }
                if name == "[]=" && args.len() == 3 {
                    check_unfrozen(self)?;
                    if let (Value::Int(i), Value::Int(n), Value::Str(repl)) = (&args[0], &args[1], &args[2]) {
                        let chars: Vec<char> = s.borrow().chars().collect();
                        let len = chars.len() as i64;
                        let start_raw = if *i < 0 { len + *i } else { *i };
                        if start_raw < 0 || start_raw > len || *n < 0 {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("index {i} out of string"),
                            }));
                        }
                        let start = start_raw as usize;
                        let take = (*n as usize).min(chars.len() - start);
                        let mut buf: String = chars[..start].iter().collect();
                        buf.push_str(&repl.borrow());
                        buf.extend(chars[start + take..].iter());
                        *s.borrow_mut() = buf;
                        return Ok(Some(args[2].clone()));
                    }
                    return Ok(None);
                }
                match (name, args) {
                    ("chars", []) => {
                        let elems: Vec<Value> = s.borrow().chars()
                            .map(|c| Value::new_str(c.to_string()))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("split", []) => {
                        // No-arg `split` matches CRuby's `split(nil)`:
                        // splits on runs of whitespace, drops the
                        // leading empty token.
                        let elems: Vec<Value> = s.borrow().split_whitespace()
                            .map(Value::new_str)
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("split", [Value::Str(sep)]) => {
                        let elems: Vec<Value> = if sep.borrow().is_empty() {
                            // CRuby: empty-sep split returns each character.
                            s.borrow().chars().map(|c| Value::new_str(c.to_string())).collect()
                        } else {
                            s.borrow().split(&*sep.borrow()).map(Value::new_str).collect()
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("%", [single_arg]) => {
                        // Build the argument list. A single Array
                        // splats into positional args; everything
                        // else is a one-element list. This matches
                        // CRuby's `format`/`String#%` calling
                        // convention.
                        let owned;
                        let fmt_args: &[Value] = match single_arg {
                            Value::Array(arr_id) => {
                                owned = self.heap.array(*arr_id).clone();
                                owned.as_slice()
                            }
                            _ => std::slice::from_ref(single_arg),
                        };
                        let out = ruby_sprintf(&s.borrow(), fmt_args, &self.heap, &self.interner)
                            .map_err(|e| self.trap(e))?;
                        if let Some(max) = self.max_value_bytes
                            && out.len() > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("String#% would exceed {max} bytes"),
                                }));
                            }
                        Some(Value::new_str(out))
                    }
                    // Literal-substring `scan` — returns a fresh
                    // Array containing one copy of the pattern for
                    // every non-overlapping occurrence in the
                    // receiver. CRuby's full `scan` accepts a
                    // Regexp and yields capture groups; literal
                    // patterns are the degenerate case where every
                    // match is the pattern itself, exactly what we
                    // implement. An empty pattern returns
                    // `[""] * (chars + 1)` to match CRuby; this is
                    // unusual but well-defined and cheap.
                    ("scan", [Value::Str(pat)]) => {
                        let parts: Vec<Value> = if pat.borrow().is_empty() {
                            std::iter::repeat_with(|| Value::new_str(""))
                                .take(s.borrow().chars().count() + 1)
                                .collect()
                        } else {
                            let mut out: Vec<Value> = Vec::new();
                            let mut i = 0;
                            let s_ref = s.borrow();
                            let bytes = s_ref.as_bytes();
                            let pat_ref = pat.borrow();
                            let plen = pat_ref.len();
                            while i + plen <= bytes.len() {
                                if &bytes[i..i + plen] == pat_ref.as_bytes() {
                                    out.push(Value::Str(pat.clone()));
                                    i += plen;
                                } else {
                                    i += 1;
                                }
                            }
                            out
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(parts));
                        Some(Value::Array(id))
                    }
                    ("to_sym", []) => {
                        // P2-14b: cap the interner before a hot loop
                        // (`arr.map { |x| x.to_s.to_sym }` and similar)
                        // can quietly grow it without bound. Existing
                        // symbols always re-resolve; only fresh strings
                        // count against the cap.
                        if let Some(max) = self.max_symbols
                            && !self.interner.contains(&s.borrow()) && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                        let sym = self.interner.intern(&s.borrow());
                        Some(Value::Sym(sym))
                    }
                    _ => None,
                }
        })
    }
}

/// Ruby's `String#succ` / `#next` — the "alphanumeric successor".
/// Walks right-to-left looking for the first alnum char, increments
/// it; on rollover ('z'→'a', 'Z'→'A', '9'→'0') carries into the
/// next char left. If the leftmost alnum rolls over, a new char of
/// the same class is prepended ('z' → 'aa', '9' → '10', 'Az' → 'Ba'
/// — wait actually 'Az' → 'Ba'? Yes: carry pushes 'A'→'B').
///
/// Used both directly (`String#succ` primitive) and by Range#each
/// over String endpoints for the canonical `('a'..'z').to_a`
/// iteration. CRuby's full spec covers a few more edge cases
/// (bracketed-string forms, all-non-alnum) which we don't reach
/// in the subset; those return the input unchanged.
pub(crate) fn str_succ(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = chars.clone();
    let mut carry_kind: Option<char> = None; // 'a' / 'A' / '0' if we ran off the front

    let mut i = out.len();
    loop {
        if i == 0 {
            // Walked past the leftmost char with the carry still pending —
            // prepend a fresh char of the same class.
            if let Some(k) = carry_kind {
                out.insert(0, k);
            }
            return out.into_iter().collect();
        }
        i -= 1;
        let c = out[i];
        match c {
            'a'..='y' | 'A'..='Y' | '0'..='8' => {
                out[i] = (c as u8 + 1) as char;
                return out.into_iter().collect();
            }
            'z' => { out[i] = 'a'; carry_kind = Some('a'); /* continue carry */ }
            'Z' => { out[i] = 'A'; carry_kind = Some('A'); }
            '9' => { out[i] = '0'; carry_kind = Some('1'); }
            _ => {
                // Non-alnum: no increment here; if we were in a carry,
                // CRuby pushes a fresh char of the carry class in front
                // of the current position. We just continue scanning
                // — eventually we run off the front and insert. For
                // pure-non-alnum inputs this returns the input unchanged,
                // matching CRuby for the common subset.
                if carry_kind.is_some() { continue; }
                // No alnum found yet — just bump this char's byte.
                // CRuby's behaviour here is "use the rightmost char's
                // succ", which for non-alnum bytes is byte+1. Good
                // enough for the niche.
                out[i] = (c as u32 + 1) as u8 as char;
                return out.into_iter().collect();
            }
        }
    }
}

/// Translate Ruby's `\0` / `\1` / … backref syntax in a
/// String#gsub replacement template into the `regex` crate's
/// `$0` / `$1` / … convention. Doubled backslash (`\\`) escapes
/// a literal backslash. `\&` is the entire match (CRuby alias
/// for `\0`); `\'` (post-match) / `\`` (pre-match) are NOT
/// supported in our subset — they'd need MatchData state we
/// don't currently carry.
///
/// Also escapes any literal `$` in the template so the regex
/// crate doesn't interpret it as its own backref form.
pub(crate) fn ruby_backref_to_dollar(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some(&n) if n.is_ascii_digit() => {
                    chars.next();
                    out.push('$');
                    out.push(n);
                }
                Some(&'&') => {
                    chars.next();
                    out.push('$');
                    out.push('0');
                }
                Some(&'\\') => {
                    chars.next();
                    out.push('\\');
                }
                _ => out.push('\\'),
            },
            // Escape `$` so the regex crate doesn't capture it.
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}
