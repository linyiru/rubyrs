//! `_yaml_native` — native fast path for the blessed-reimpl YAML
//! loader (ADR 0026).
//!
//! ADR 0019 Rule 6 partition, with a twist: unlike `_json_native` /
//! `_rouge_native` / `_kramdown_native` (which accelerate REAL gems),
//! the spec here is rubyrs' own `stdlib_vendor/yaml.rb` — a 317-line
//! focused front-matter loader whose semantics are already pinned by
//! the byte-identical Jekyll builds. This file is a 1:1 translation of
//! `RubyrsYAMLParse` (preprocess / strip_trailing_comment /
//! parse_block / parse_mapping / parse_sequence / split_key / scalar /
//! quoted / flow), bugs and all — any intentional divergence would
//! break the pure-vs-native differential contract.
//!
//! Host fn:
//!   - `__rubyrs_yaml_parse(src) → value` — parse a YAML document into
//!     VM values directly (same materialization shape as
//!     `_json_native`'s visitor). Raises on inputs the translation
//!     cannot reproduce exactly (non-UTF-8 source, integers beyond
//!     i64, pathological nesting) — `yaml.rb` rescues and falls back
//!     to the pure-Ruby path, so behaviour is unchanged there.

#![cfg(feature = "_yaml_native")]

use crate::error::{RubyError, Trap};
use crate::heap::{HashObj, HeapObj};
use crate::value::Value;
use crate::vm::current_vm_ptr;

/// Recursion cap for nested block/flow structures. Front matter is
/// shallow; anything deeper declines to the pure path (whose VM stack
/// guard handles it) rather than risking the Rust stack.
const MAX_DEPTH: usize = 256;

fn decline(msg: &str) -> Trap {
    Trap {
        err: RubyError::RuntimeError {
            msg: format!("yaml_native: {msg}"),
        },
        backtrace: vec![],
    }
}

/// Register the `__rubyrs_yaml_parse` host fn on `rt`. Idempotent.
/// `stdlib_vendor/yaml.rb` detects registration via `defined?(...)`
/// and routes `parse_document` through it.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    rt.register_fn("__rubyrs_yaml_parse", |args| {
        let src = match args {
            [Value::Str(s)] => s,
            _ => {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: "__rubyrs_yaml_parse(src: String)".to_string(),
                    },
                    backtrace: vec![],
                });
            }
        };
        let bytes = src.borrow();
        // The pure loader works on Ruby's byte-oriented strings; the
        // translation is `char`-indexed UTF-8. Divergent ground →
        // decline (rescue + pure fallback in yaml.rb).
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Err(decline("non-UTF-8 source"));
        };
        let ptr = current_vm_ptr();
        if ptr.is_null() {
            return Err(decline("CURRENT_VM_PTR null"));
        }
        // SAFETY: set by the dispatch site immediately before this
        // closure runs; the &mut borrow lasts only for this
        // synchronous call and isn't stashed (same shape as
        // json_native's visitor).
        let vm = unsafe { &mut *ptr };
        parse_document(vm, text)
    });
}

// ---- 1:1 translation of RubyrsYAMLParse --------------------------------

fn parse_document(vm: &mut crate::vm::Vm, source: &str) -> Result<Value, Trap> {
    let lines = preprocess(source);
    if lines.is_empty() {
        return Ok(Value::Nil);
    }
    let mut idx = 0;
    parse_block(vm, &lines, &mut idx, 0, 0)
}

/// Ruby `String#strip` trims " \t\n\v\f\r\0" — NOT Unicode whitespace
/// (`str::trim` would also eat U+00A0 etc. and silently diverge).
fn ruby_strip(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\0'))
}

/// Strip a leading `---`, stop at a trailing `...`, drop blank and
/// comment-only lines, strip trailing comments. Returns
/// `[(indent, content), ...]` — indent counts LEADING SPACES of the
/// comment-stripped line (tabs don't count, mirroring `[/\A */]`).
fn preprocess(src: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for line in src.split('\n') {
        let stripped = ruby_strip(line);
        if stripped == "---" {
            continue;
        }
        if stripped == "..." {
            break;
        }
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with('#') {
            continue;
        }
        let content = strip_trailing_comment(line);
        let content_stripped = ruby_strip(content);
        if content_stripped.is_empty() {
            continue;
        }
        let indent = content.chars().take_while(|c| *c == ' ').count();
        out.push((indent, content_stripped));
    }
    out
}

/// Cut the line at an unquoted ` #` / `\t#` comment marker.
fn strip_trailing_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let mut prev = ' ';
    for (byte_pos, c) in line.char_indices() {
        if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if c == '#' && !in_single && !in_double && (prev == ' ' || prev == '\t') {
            return &line[..byte_pos];
        }
        prev = c;
    }
    line
}

fn parse_block(
    vm: &mut crate::vm::Vm,
    lines: &[(usize, &str)],
    idx: &mut usize,
    min_indent: usize,
    depth: usize,
) -> Result<Value, Trap> {
    if depth > MAX_DEPTH {
        return Err(decline("nesting too deep"));
    }
    if *idx >= lines.len() {
        return Ok(Value::Nil);
    }
    let (indent, content) = lines[*idx];
    if indent < min_indent {
        return Ok(Value::Nil);
    }
    if content.starts_with("- ") || content == "-" {
        parse_sequence(vm, lines, idx, indent, depth)
    } else if split_key(content).is_some() {
        parse_mapping(vm, lines, idx, indent, depth)
    } else {
        *idx += 1;
        scalar(vm, content, depth)
    }
}

fn parse_mapping(
    vm: &mut crate::vm::Vm,
    lines: &[(usize, &str)],
    idx: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Value, Trap> {
    let mut pairs: Vec<(Value, Value)> = Vec::new();
    while *idx < lines.len() {
        let (cur_indent, content) = lines[*idx];
        if cur_indent != indent {
            break;
        }
        let Some((key, rest)) = split_key(content) else {
            break;
        };
        *idx += 1;
        let k = scalar(vm, key, depth)?;
        let v = if rest.is_empty() {
            if *idx < lines.len() && lines[*idx].0 > indent {
                parse_block(vm, lines, idx, indent + 1, depth + 1)?
            } else if *idx < lines.len()
                && lines[*idx].0 == indent
                && (lines[*idx].1.starts_with("- ") || lines[*idx].1 == "-")
            {
                parse_sequence(vm, lines, idx, indent, depth + 1)?
            } else {
                Value::Nil
            }
        } else {
            scalar(vm, rest, depth)?
        };
        hash_insert(&mut pairs, k, v);
    }
    Ok(alloc_hash(vm, pairs))
}

fn parse_sequence(
    vm: &mut crate::vm::Vm,
    lines: &[(usize, &str)],
    idx: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Value, Trap> {
    let mut seq: Vec<Value> = Vec::new();
    while *idx < lines.len() {
        let (cur_indent, content) = lines[*idx];
        if cur_indent < indent {
            break;
        }
        if !(content.starts_with("- ") || content == "-") {
            break;
        }
        let item = if content == "-" { "" } else { &content[2..] };
        *idx += 1;
        if ruby_strip(item).is_empty() {
            if *idx < lines.len() && lines[*idx].0 > indent {
                seq.push(parse_block(vm, lines, idx, indent + 1, depth + 1)?);
            } else {
                seq.push(Value::Nil);
            }
        } else if let Some((key, rest)) = split_key(item) {
            // Inline-map item: `- key: value` plus following
            // `indent+2`-or-deeper key lines absorbed into the map.
            let mut pairs: Vec<(Value, Value)> = Vec::new();
            let k = scalar(vm, key, depth)?;
            let v = if rest.is_empty() {
                Value::Nil
            } else {
                scalar(vm, rest, depth)?
            };
            hash_insert(&mut pairs, k, v);
            let item_indent = indent + 2;
            while *idx < lines.len() && lines[*idx].0 >= item_indent {
                let Some((ik, ir)) = split_key(lines[*idx].1) else {
                    break;
                };
                *idx += 1;
                let ikv = scalar(vm, ik, depth)?;
                let irv = if ir.is_empty() {
                    Value::Nil
                } else {
                    scalar(vm, ir, depth)?
                };
                hash_insert(&mut pairs, ikv, irv);
            }
            seq.push(alloc_hash(vm, pairs));
        } else {
            seq.push(scalar(vm, item, depth)?);
        }
    }
    let id = vm.heap.alloc(HeapObj::Array(seq));
    Ok(Value::Array(id))
}

/// Find the top-level `key:` split (colon + space or EOL), respecting
/// quotes and flow brackets. Returns `(key.strip, rest.strip)`.
fn split_key(content: &str) -> Option<(&str, &str)> {
    let mut in_single = false;
    let mut in_double = false;
    let mut flow_depth: i64 = 0;
    let mut iter = content.char_indices().peekable();
    while let Some((byte_pos, c)) = iter.next() {
        if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if !in_single && !in_double {
            match c {
                '[' | '{' => flow_depth += 1,
                ']' | '}' => flow_depth -= 1,
                ':' if flow_depth == 0 => {
                    let next = iter.peek().map(|(_, n)| *n);
                    if next == Some(' ') || next.is_none() {
                        let key = ruby_strip(&content[..byte_pos]);
                        let rest = ruby_strip(&content[byte_pos + 1..]);
                        return Some((key, rest));
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn scalar(vm: &mut crate::vm::Vm, s: &str, depth: usize) -> Result<Value, Trap> {
    if depth > MAX_DEPTH {
        return Err(decline("nesting too deep"));
    }
    let s = ruby_strip(s);
    if s.is_empty() || s == "~" || s == "null" || s == "Null" || s == "NULL" {
        return Ok(Value::Nil);
    }
    if s.starts_with('"') {
        return Ok(Value::new_str(parse_double_quoted(s)));
    }
    if s.starts_with('\'') {
        return Ok(Value::new_str(parse_single_quoted(s)));
    }
    if s.starts_with('[') {
        return parse_flow_seq(vm, s, depth);
    }
    if s.starts_with('{') {
        return parse_flow_map(vm, s, depth);
    }
    match s {
        "true" | "True" | "TRUE" => return Ok(Value::Bool(true)),
        "false" | "False" | "FALSE" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if is_int_literal(s) {
        // Ruby `to_i` promotes past i64 to Bignum — out of scope for
        // the translation; decline so the pure path handles it.
        return match s.parse::<i64>() {
            Ok(n) => Ok(Value::Int(n)),
            Err(_) => Err(decline("integer beyond i64")),
        };
    }
    if is_float_literal(s) || is_sci_literal(s) {
        // Ruby `to_f` and Rust `f64::parse` are both
        // correctly-rounded strtod; overflow → ±inf in both.
        return match s.parse::<f64>() {
            Ok(f) => Ok(Value::Float(f)),
            Err(_) => Err(decline("unparsable float literal")),
        };
    }
    Ok(Value::new_str(s.to_string()))
}

/// `/\A[-+]?\d+\z/`
fn is_int_literal(s: &str) -> bool {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// `/\A[-+]?\d+\.\d+\z/`
fn is_float_literal(s: &str) -> bool {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    let Some((int_part, frac_part)) = body.split_once('.') else {
        return false;
    };
    !int_part.is_empty()
        && !frac_part.is_empty()
        && int_part.bytes().all(|b| b.is_ascii_digit())
        && frac_part.bytes().all(|b| b.is_ascii_digit())
}

/// `/\A[-+]?\d+(\.\d+)?[eE][-+]?\d+\z/`
fn is_sci_literal(s: &str) -> bool {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    let Some(e_pos) = body.find(['e', 'E']) else {
        return false;
    };
    let mantissa = &body[..e_pos];
    let exponent = &body[e_pos + 1..];
    let exponent = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
    if exponent.is_empty() || !exponent.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match mantissa.split_once('.') {
        Some((i, f)) => {
            !i.is_empty()
                && !f.is_empty()
                && i.bytes().all(|b| b.is_ascii_digit())
                && f.bytes().all(|b| b.is_ascii_digit())
        }
        None => !mantissa.is_empty() && mantissa.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// Ruby `s[1..-2].to_s` — drop the first and last CHAR without
/// verifying the closer (the pure loader doesn't either; `"abc`
/// becomes `ab`). Single-char input → "".
fn inner_slice(s: &str) -> &str {
    let mut chars = s.char_indices();
    let Some((_, first)) = chars.next() else {
        return "";
    };
    let start = first.len_utf8();
    match s.char_indices().next_back() {
        Some((last_pos, _)) if last_pos >= start => &s[start..last_pos],
        _ => "",
    }
}

fn parse_double_quoted(s: &str) -> String {
    let inner = inner_slice(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&n) = chars.peek()
        {
            chars.next();
            out.push(match n {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '"' => '"',
                '\\' => '\\',
                '0' => '\0',
                other => other,
            });
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_single_quoted(s: &str) -> String {
    inner_slice(s).replace("''", "'")
}

fn parse_flow_seq(vm: &mut crate::vm::Vm, s: &str, depth: usize) -> Result<Value, Trap> {
    let inner = ruby_strip(inner_slice(s));
    let mut elems = Vec::new();
    if !inner.is_empty() {
        for part in split_flow(inner) {
            elems.push(scalar(vm, part, depth + 1)?);
        }
    }
    let id = vm.heap.alloc(HeapObj::Array(elems));
    Ok(Value::Array(id))
}

fn parse_flow_map(vm: &mut crate::vm::Vm, s: &str, depth: usize) -> Result<Value, Trap> {
    let inner = ruby_strip(inner_slice(s));
    let mut pairs: Vec<(Value, Value)> = Vec::new();
    if !inner.is_empty() {
        for part in split_flow(inner) {
            if let Some((key, rest)) = split_key(part) {
                let k = scalar(vm, key, depth + 1)?;
                let v = if rest.is_empty() {
                    Value::Nil
                } else {
                    scalar(vm, rest, depth + 1)?
                };
                hash_insert(&mut pairs, k, v);
            }
        }
    }
    Ok(alloc_hash(vm, pairs))
}

/// Split flow content on top-level commas, respecting quotes and
/// nested brackets. Returns stripped parts (including a trailing empty
/// part for `[a,]`, mirroring the pure loader).
fn split_flow(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut flow_depth: i64 = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut start = 0;
    for (byte_pos, c) in s.char_indices() {
        if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if !in_single && !in_double {
            match c {
                '[' | '{' => flow_depth += 1,
                ']' | '}' => flow_depth -= 1,
                ',' if flow_depth == 0 => {
                    parts.push(ruby_strip(&s[start..byte_pos]));
                    start = byte_pos + 1;
                }
                _ => {}
            }
        }
    }
    parts.push(ruby_strip(&s[start..]));
    parts
}

/// `map[k] = v` — Ruby Hash assignment REPLACES an existing key's
/// value in place (insertion order preserved). The pure loader gets
/// this from Ruby Hash; a plain `pairs.push` would produce duplicate
/// keys in `HashObj::with_pairs`.
fn hash_insert(pairs: &mut Vec<(Value, Value)>, k: Value, v: Value) {
    if let Some(slot) = pairs.iter_mut().find(|(pk, _)| yaml_key_eq(pk, &k)) {
        slot.1 = v;
    } else {
        pairs.push((k, v));
    }
}

/// Key equality for the scalar key types this loader can produce
/// (String / Int / Float / Bool / Nil — flow collections never become
/// KEYS because `split_key` strips and `scalar` on a key only sees
/// scalar shapes in practice; collection keys compare conservatively
/// as never-equal, which only affects pathological duplicate-key
/// docs).
fn yaml_key_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => *x.borrow() == *y.borrow(),
        _ => false,
    }
}

fn alloc_hash(vm: &mut crate::vm::Vm, pairs: Vec<(Value, Value)>) -> Value {
    let id = vm.heap.alloc(HeapObj::Hash(HashObj::with_pairs(pairs)));
    Value::Hash(id)
}
