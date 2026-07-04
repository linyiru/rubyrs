//! Native whitequark translation — port of the prism gem's
//! `Prism::Translation::Parser#tokenize` pipeline to Rust (the "wqtrans" seam).
//!
//! RuboCop's prism engine parses via `Prism::Translation::Parser34#tokenize`,
//! which visits the prism tree with the gem's interpreted
//! `translation/parser/compiler.rb` (+ the parser gem's `Builders::Default`)
//! and translates the prism token stream with `translation/parser/lexer.rb`.
//! That interpreted visit dominates RuboCop's per-file parse cost on rubyrs
//! (~65ms of a 67ms parse for a 600-line file). This module does the whole
//! translation natively:
//!
//! - `mod.rs` (here): decodes the linked prism C library's serialize-parse-lex
//!   wire blob into a generic Rust tree (`PNode`, field order pinned by the
//!   GENERATED `ids.rs`, same single-source-of-truth discipline as
//!   `prism_node_specs.rs`), plus tokens/comments/errors/warnings; then runs
//!   the ported compiler + lexer and materializes the result as the exact Ruby
//!   object graph the interpreted path builds (`RuboCop::AST::*` nodes,
//!   `Parser::Source::Map*` maps, `Parser::Source::Range`s tied to the
//!   caller's `Parser::Source::Buffer`, `Parser::Source::Comment`s, parser-gem
//!   token triples, and diagnostic rows the Ruby hook replays through
//!   `parser.diagnostics.process`).
//! - `compiler.rs` / `builder.rs`: faithful ports of
//!   `translation/parser/compiler.rb` (prism 1.9.0) and the subset of
//!   `parser/builders/default.rb` (parser 3.3.7.0) it exercises, specialized
//!   to the flag configuration RuboCop's `BuilderPrism` runs with (verified at
//!   the Ruby hook: emit_forward_arg=true, emit_match_pattern=true, all other
//!   emit_* false, emit_file_line_as_literals=true).
//! - `lexer.rs`: port of `translation/parser/lexer.rb`.
//! - `materialize.rs`: Rust tree → rubyrs heap objects.
//!
//! Decline-don't-crash: any construct the port doesn't cover returns
//! `Err(Decline)`; the host fn surfaces `nil` and the Ruby hook falls back to
//! the interpreted translation for that file (whose behavior is the spec).
//! `RUBYRS_WQTRANS_NO_NATIVE=1` is the kill switch.
//!
//! GC safety: `Heap::alloc` never collects mid-host-fn (same audited invariant
//! prism_materialize.rs and json_native.rs rely on), so `Value`s held in Rust
//! structures while the tree is being built cannot be swept. On decline the
//! half-built subgraph is unrooted garbage, reclaimed by the next collection.

pub(crate) mod ids;
mod builder;
mod compiler;
mod lexer;
mod materialize;

/// The Ruby-side hook, injected by the `require` handler right after
/// "prism/translation/parser" loads (see vm/kernel.rs): overrides
/// `Prism::Translation::Parser#tokenize` with native-first + per-file
/// fallback. wasi-gated with that handler: `require` raises LoadError
/// on wasm32-wasi, so the injection site is cfg'd out there and the
/// const would be dead code.
#[cfg(not(target_os = "wasi"))]
pub(crate) const HOOK_RB: &str = include_str!("wqtrans_hook.rb");

use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::prism_materialize::FieldKind;
use crate::prism_node_specs::{NODE_SPECS, WIRE_VERSION};
use crate::value::{EncodingTag, Value};
use crate::vm::Vm;

// ---------------------------------------------------------------------------
// Decoded prism tree (generic, field order per ids.rs)
// ---------------------------------------------------------------------------

/// One decoded prism node: wire type id + flags + byte-offset location +
/// positional fields (order pinned by `ids.rs`).
pub(crate) struct PNode {
    pub(crate) ty: u8,
    pub(crate) flags: u32,
    /// Byte offsets into the source.
    pub(crate) loc: (u32, u32),
    pub(crate) fields: Box<[PField]>,
}

pub(crate) enum PField {
    Node(Box<PNode>),
    OptNode(Option<Box<PNode>>),
    List(Vec<PNode>),
    /// 0-based constant-pool index.
    Const(u32),
    OptConst(Option<u32>),
    /// Decoded for cursor correctness; the values (locals tables) are not
    /// consulted by the translation.
    ConstList(#[allow(dead_code)] Vec<u32>),
    /// Decoded string bytes (source slice or embedded).
    Str(Vec<u8>),
    Loc(u32, u32),
    OptLoc(Option<(u32, u32)>),
    UInt(u64),
    Int(PInt),
    Double(f64),
}

pub(crate) enum PInt {
    Small(i64),
    #[cfg(feature = "bignum")]
    Big(num_bigint::BigInt),
}

impl PNode {
    pub(crate) fn node(&self, i: usize) -> Option<&PNode> {
        // Accepts OptNode fields too: several "required in practice" fields
        // (case_match predicate, ...) are optional on the wire; a truly-nil
        // one returns None and the caller declines (where Ruby would crash).
        match self.fields.get(i)? {
            PField::Node(n) => Some(n),
            PField::OptNode(n) => n.as_deref(),
            _ => None,
        }
    }
    pub(crate) fn opt_node(&self, i: usize) -> Option<&PNode> {
        match self.fields.get(i) {
            Some(PField::OptNode(n)) => n.as_deref(),
            _ => None,
        }
    }
    pub(crate) fn list(&self, i: usize) -> &[PNode] {
        match self.fields.get(i) {
            Some(PField::List(v)) => v,
            _ => &[],
        }
    }
    pub(crate) fn cid(&self, i: usize) -> Option<u32> {
        match self.fields.get(i)? {
            PField::Const(c) => Some(*c),
            PField::OptConst(c) => *c,
            _ => None,
        }
    }
    pub(crate) fn str_bytes(&self, i: usize) -> Option<&[u8]> {
        match self.fields.get(i)? {
            PField::Str(b) => Some(b),
            _ => None,
        }
    }
    /// Required location field — byte offsets.
    pub(crate) fn bloc(&self, i: usize) -> Option<(u32, u32)> {
        match self.fields.get(i)? {
            PField::Loc(b, e) => Some((*b, *e)),
            PField::OptLoc(l) => *l,
            _ => None,
        }
    }
    pub(crate) fn opt_bloc(&self, i: usize) -> Option<(u32, u32)> {
        match self.fields.get(i) {
            Some(PField::Loc(b, e)) => Some((*b, *e)),
            Some(PField::OptLoc(l)) => *l,
            _ => None,
        }
    }
    pub(crate) fn uint(&self, i: usize) -> Option<u64> {
        match self.fields.get(i)? {
            PField::UInt(n) => Some(*n),
            _ => None,
        }
    }
    pub(crate) fn double(&self, i: usize) -> Option<f64> {
        match self.fields.get(i)? {
            PField::Double(d) => Some(*d),
            _ => None,
        }
    }
    pub(crate) fn int(&self, i: usize) -> Option<&PInt> {
        match self.fields.get(i)? {
            PField::Int(n) => Some(n),
            _ => None,
        }
    }
}

/// Decoded prism token row (parse_lex wire order).
#[derive(Clone, Copy)]
pub(crate) struct PTok {
    /// Index into `prism_node_specs::TOKEN_TYPES`.
    pub(crate) ty: u16,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) lex_state: u32,
}

pub(crate) struct PComment {
    /// 0 = inline, 1 = embdoc — both become `Parser::Source::Comment`s of the
    /// same shape; kept for decode clarity.
    #[allow(dead_code)]
    pub(crate) kind: u8,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

pub(crate) struct PDiag {
    /// Index into `prism_node_specs::DIAGNOSTIC_TYPES`.
    pub(crate) ty: u16,
    pub(crate) message: Vec<u8>,
    pub(crate) start: u32,
    pub(crate) end: u32,
    /// Level byte (errors: 0=syntax 1=argument 2=load; warnings: 0=default 1=verbose).
    #[allow(dead_code)]
    pub(crate) level: u8,
}

pub(crate) struct PParse {
    pub(crate) tokens: Vec<PTok>,
    pub(crate) comments: Vec<PComment>,
    pub(crate) errors: Vec<PDiag>,
    pub(crate) warnings: Vec<PDiag>,
    pub(crate) root: PNode,
    /// Constant pool decoded lazily: (pool base offset, count).
    cpool_base: usize,
    cpool_len: usize,
    pub(crate) enc: EncodingTag,
}

// ---------------------------------------------------------------------------
// Wire decoding (mirrors prism_materialize.rs's Reader, into Rust structs)
// ---------------------------------------------------------------------------

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn read(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.buf.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn varuint(&mut self) -> Option<u64> {
        let mut n = self.u8()? as u64;
        if n < 128 {
            return Some(n);
        }
        n -= 128;
        let mut shift = 0u32;
        loop {
            let b = self.u8()? as u64;
            shift += 7;
            if shift > 63 {
                return None;
            }
            if b >= 128 {
                n += (b - 128) << shift;
            } else {
                return Some(n + (b << shift));
            }
        }
    }
    fn varsint(&mut self) -> Option<i64> {
        let n = self.varuint()?;
        Some(((n >> 1) as i64) ^ -((n & 1) as i64))
    }
    fn u32_native(&mut self) -> Option<u32> {
        let b = self.read(4)?;
        Some(u32::from_ne_bytes(b.try_into().ok()?))
    }
    fn f64_native(&mut self) -> Option<f64> {
        let b = self.read(8)?;
        Some(f64::from_ne_bytes(b.try_into().ok()?))
    }
    fn loc32(&mut self) -> Option<(u32, u32)> {
        let start = self.varuint()?;
        let len = self.varuint()?;
        if start > u32::MAX as u64 || start + len > u32::MAX as u64 {
            return None;
        }
        Some((start as u32, (start + len) as u32))
    }
}

fn decode_str(r: &mut Reader<'_>, input: &[u8]) -> Option<Vec<u8>> {
    match r.u8()? {
        1 => {
            let start = r.varuint()? as usize;
            let length = r.varuint()? as usize;
            Some(input.get(start..start.checked_add(length)?)?.to_vec())
        }
        2 => {
            let length = r.varuint()? as usize;
            Some(r.read(length)?.to_vec())
        }
        _ => None,
    }
}

fn decode_integer(r: &mut Reader<'_>) -> Option<PInt> {
    let negative = r.u8()? != 0;
    let len = r.varuint()? as usize;
    let mut digits = Vec::with_capacity(len);
    for _ in 0..len {
        let chunk = r.varuint()?;
        if chunk > u32::MAX as u64 {
            return None;
        }
        digits.push(chunk as u32);
    }
    if len <= 2 {
        let mut value: u128 = 0;
        for (i, d) in digits.iter().enumerate() {
            value |= (*d as u128) << (32 * i);
        }
        let signed = if negative { -(value as i128) } else { value as i128 };
        if let Ok(n) = i64::try_from(signed) {
            return Some(PInt::Small(n));
        }
    }
    #[cfg(feature = "bignum")]
    {
        let sign = if negative { num_bigint::Sign::Minus } else { num_bigint::Sign::Plus };
        let b = num_bigint::BigInt::from_slice(sign, &digits);
        if let Ok(n) = i64::try_from(&b) {
            return Some(PInt::Small(n));
        }
        return Some(PInt::Big(b));
    }
    #[allow(unreachable_code)]
    None
}

fn decode_node(r: &mut Reader<'_>, input: &[u8], depth: u32) -> Option<PNode> {
    // Defense against pathological nesting blowing the Rust stack; the
    // interpreted path handles such files (slowly) instead.
    if depth > 4096 {
        return None;
    }
    let ty = r.u8()?;
    let spec = NODE_SPECS.get((ty as usize).checked_sub(1)?)?;
    let _node_id = r.varuint()?;
    let loc = r.loc32()?;
    if spec.skip_uint32 {
        r.read(4)?;
    }
    let flags = r.varuint()?;
    if flags > u32::MAX as u64 {
        return None;
    }

    let mut fields = Vec::with_capacity(spec.fields.len());
    for (kind, _) in spec.fields {
        let f = match kind {
            FieldKind::Node => PField::Node(Box::new(decode_node(r, input, depth + 1)?)),
            FieldKind::OptNode => {
                if *r.buf.get(r.pos)? == 0 {
                    r.pos += 1;
                    PField::OptNode(None)
                } else {
                    PField::OptNode(Some(Box::new(decode_node(r, input, depth + 1)?)))
                }
            }
            FieldKind::NodeList => {
                let n = r.varuint()? as usize;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(decode_node(r, input, depth + 1)?);
                }
                PField::List(items)
            }
            FieldKind::Constant => PField::Const(u32::try_from(r.varuint()?.checked_sub(1)?).ok()?),
            FieldKind::OptConstant => {
                let idx = r.varuint()?;
                PField::OptConst(if idx == 0 { None } else { Some(u32::try_from(idx - 1).ok()?) })
            }
            FieldKind::ConstantList => {
                let n = r.varuint()? as usize;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(u32::try_from(r.varuint()?.checked_sub(1)?).ok()?);
                }
                PField::ConstList(items)
            }
            FieldKind::Str => PField::Str(decode_str(r, input)?),
            FieldKind::Location => {
                let (b, e) = r.loc32()?;
                PField::Loc(b, e)
            }
            FieldKind::OptLocation => {
                if r.u8()? == 0 {
                    PField::OptLoc(None)
                } else {
                    PField::OptLoc(Some(r.loc32()?))
                }
            }
            FieldKind::VarUint => PField::UInt(r.varuint()?),
            FieldKind::UInt8 => PField::UInt(r.u8()? as u64),
            FieldKind::Integer => PField::Int(decode_integer(r)?),
            FieldKind::Double => PField::Double(r.f64_native()?),
        };
        fields.push(f);
    }

    Some(PNode { ty, flags: flags as u32, loc, fields: fields.into_boxed_slice() })
}

/// Decode a `pm_serialize_parse_lex` blob. Returns `None` (decline) on any
/// unexpected shape.
fn decode_parse_lex(blob: &[u8], input: &[u8], vm: &Vm) -> Option<PParse> {
    let mut r = Reader { buf: blob, pos: 0 };

    // Tokens come first on the parse_lex wire.
    let mut tokens = Vec::new();
    loop {
        let ty = r.varuint()?;
        if ty == 0 {
            break;
        }
        let start = r.varuint()?;
        let length = r.varuint()?;
        let lex_state = r.varuint()?;
        if start + length > u32::MAX as u64 {
            return None;
        }
        tokens.push(PTok {
            ty: u16::try_from(ty).ok()?,
            start: start as u32,
            end: (start + length) as u32,
            lex_state: u32::try_from(lex_state).ok()?,
        });
    }

    // Header: magic + version pin + location-fields byte.
    if r.read(5)? != b"PRISM" {
        return None;
    }
    if (r.u8()?, r.u8()?, r.u8()?) != WIRE_VERSION {
        return None;
    }
    if r.u8()? != 0 {
        return None;
    }

    // Encoding row.
    let n = r.varuint()? as usize;
    let enc_name = std::str::from_utf8(r.read(n)?).ok()?;
    let enc = Vm::encoding_tag_from_str(enc_name)?;

    // Source line table (start_line + newline offsets) — recomputed from the
    // source bytes on our side, skip.
    let _start_line = r.varsint()?;
    let offset_count = r.varuint()? as usize;
    for _ in 0..offset_count {
        r.varuint()?;
    }

    // Comments.
    let n = r.varuint()? as usize;
    let mut comments = Vec::with_capacity(n);
    for _ in 0..n {
        let kind = r.varuint()?;
        let (start, end) = r.loc32()?;
        comments.push(PComment { kind: u8::try_from(kind).ok()?, start, end });
    }

    // Magic comments (unused by the translation) + data_loc.
    let n = r.varuint()? as usize;
    for _ in 0..n {
        r.loc32()?;
        r.loc32()?;
    }
    if r.u8()? != 0 {
        r.loc32()?;
    }

    // Errors and warnings.
    let decode_diags = |r: &mut Reader<'_>| -> Option<Vec<PDiag>> {
        let n = r.varuint()? as usize;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let ty = u16::try_from(r.varuint()?).ok()?;
            let len = r.varuint()? as usize;
            let message = r.read(len)?.to_vec();
            let (start, end) = r.loc32()?;
            let level = r.u8()?;
            out.push(PDiag { ty, message, start, end, level });
        }
        Some(out)
    };
    let errors = decode_diags(&mut r)?;
    let warnings = decode_diags(&mut r)?;

    // Constant pool base + size, then the node tree.
    let cpool_base = r.u32_native()? as usize;
    let cpool_len = r.varuint()? as usize;
    cpool_base.checked_add(cpool_len.checked_mul(8)?)?.le(&blob.len()).then_some(())?;

    let root = decode_node(&mut r, input, 0)?;

    let _ = vm;
    Some(PParse { tokens, comments, errors, warnings, root, cpool_base, cpool_len, enc })
}

// ---------------------------------------------------------------------------
// Char-offset ranges + offset cache
// ---------------------------------------------------------------------------

/// A parser-gem source range in CHARACTER offsets (the parser gem deals in
/// characters; prism in bytes — the conversion happens exactly once, at range
/// creation, like the gem's offset_cache).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct R {
    pub(crate) b: u32,
    pub(crate) e: u32,
}

impl R {
    pub(crate) fn join(self, other: R) -> R {
        R { b: self.b.min(other.b), e: self.e.max(other.e) }
    }
}

/// Byte→char offset conversion. Identity for all-ASCII (and binary) sources.
pub(crate) enum OffsetCache {
    Identity,
    Table(Vec<u32>),
}

impl OffsetCache {
    fn build(src: &[u8]) -> OffsetCache {
        if src.is_ascii() {
            return OffsetCache::Identity;
        }
        match std::str::from_utf8(src) {
            Ok(s) => {
                let mut table = Vec::with_capacity(src.len() + 1);
                let mut ch = 0u32;
                for c in s.chars() {
                    for _ in 0..c.len_utf8() {
                        table.push(ch);
                    }
                    ch += 1;
                }
                table.push(ch);
                OffsetCache::Table(table)
            }
            // Invalid UTF-8 → the buffer is binary; bytes == chars.
            Err(_) => OffsetCache::Identity,
        }
    }

    #[inline]
    pub(crate) fn c(&self, byte: u32) -> u32 {
        match self {
            OffsetCache::Identity => byte,
            OffsetCache::Table(t) => t.get(byte as usize).copied().unwrap_or_else(|| {
                t.last().copied().unwrap_or(byte)
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Decline + diagnostics rows
// ---------------------------------------------------------------------------

/// The per-file "this port doesn't cover that" escape hatch. The reason string
/// is surfaced via RUBYRS_WQTRANS_DEBUG=1 for coverage reporting.
#[derive(Debug)]
pub(crate) struct Decline(pub(crate) &'static str);

pub(crate) type CRes<T> = Result<T, Decline>;

#[inline]
pub(crate) fn decline<T>(reason: &'static str) -> CRes<T> {
    Err(Decline(reason))
}

/// One diagnostic to be replayed (in order) through the Ruby-side
/// `parser.diagnostics.process`. Mirrors `Parser::Diagnostic.new(level,
/// reason, arguments, location, highlights)` /
/// `PrismDiagnostic.new(message, level, reason, location)`.
pub(crate) struct DiagRow {
    /// true → PrismDiagnostic (carries a verbatim prism message).
    pub(crate) prism: bool,
    /// :error / :warning.
    pub(crate) level: &'static str,
    /// Parser-gem reason symbol, or the prism error/warning type.
    pub(crate) reason: String,
    /// Message for PrismDiagnostic rows.
    pub(crate) message: Option<String>,
    /// Flat (key, value-string) pairs for the arguments hash.
    pub(crate) args: Vec<(&'static str, ArgVal)>,
    pub(crate) loc: R,
    pub(crate) highlights: Vec<R>,
}

pub(crate) enum ArgVal {
    Str(String),
    Sym(String),
}

// ---------------------------------------------------------------------------
// Translation context
// ---------------------------------------------------------------------------

/// State shared by the compiler + builder ports for one tokenize call.
pub(crate) struct Ctx<'a> {
    pub(crate) vm: &'a mut Vm,
    /// Source bytes (the buffer's source string).
    pub(crate) src: &'a [u8],
    pub(crate) blob: &'a [u8],
    pub(crate) off: OffsetCache,
    /// Byte offsets where each line starts (line 0 at offset 0).
    pub(crate) line_begins: Vec<u32>,
    /// `Parser::Source::Buffer#first_line` (1 for RuboCop).
    pub(crate) first_line: i64,
    /// `Parser::Source::Buffer#name` — the `__FILE__` literal value.
    pub(crate) buffer_name: Vec<u8>,
    /// Parser version (33/34/40/41).
    pub(crate) version: i64,
    /// Parse encoding (== buffer encoding, since the translation passes
    /// `encoding: false`).
    pub(crate) enc: EncodingTag,
    pub(crate) diags: Vec<DiagRow>,
    /// `parser.pattern_variables` — a stack of declared-name frames.
    /// (`pattern_hash_keys` exists on the real parser but is only consulted
    /// by builder methods the prism compiler never calls.)
    pub(crate) pattern_vars: Vec<Vec<String>>,
    /// Constant-pool memo: 0-based index → interned bytes.
    cpool_base: usize,
    cpool_len: usize,
}

impl<'a> Ctx<'a> {
    pub(crate) fn cpool_bytes(&self, index: u32) -> Option<&'a [u8]> {
        let index = index as usize;
        if index >= self.cpool_len {
            return None;
        }
        let off = self.cpool_base.checked_add(index.checked_mul(8)?)?;
        let row = self.blob.get(off..off + 8)?;
        let start = u32::from_ne_bytes(row[0..4].try_into().ok()?);
        let length = u32::from_ne_bytes(row[4..8].try_into().ok()?) as usize;
        if start & (1 << 31) == 0 {
            self.src.get(start as usize..(start as usize).checked_add(length)?)
        } else {
            let s = (start & ((1 << 31) - 1)) as usize;
            self.blob.get(s..s.checked_add(length)?)
        }
    }

    /// Constant-pool name for a node field, as an interned Ruby Symbol.
    pub(crate) fn cname(&mut self, node: &PNode, field: usize) -> CRes<crate::intern::SymId> {
        let Some(cid) = node.cid(field) else { return decline("cname: field kind") };
        let Some(bytes) = self.cpool_bytes(cid) else { return decline("cname: pool index") };
        Ok(self.intern_bytes(&bytes.to_vec()))
    }

    /// Constant-pool name as owned bytes.
    pub(crate) fn cname_bytes(&self, node: &PNode, field: usize) -> CRes<Vec<u8>> {
        let Some(cid) = node.cid(field) else { return decline("cname_bytes: field kind") };
        let Some(bytes) = self.cpool_bytes(cid) else { return decline("cname_bytes: pool index") };
        Ok(bytes.to_vec())
    }

    /// `String#to_sym` equivalence: rubyrs's interner is str-keyed, so
    /// non-UTF-8 symbol names take the same lossy view the interpreted
    /// `to_sym` takes.
    pub(crate) fn intern_bytes(&mut self, bytes: &[u8]) -> crate::intern::SymId {
        match std::str::from_utf8(bytes) {
            Ok(s) => self.vm.interner.intern(s),
            Err(_) => {
                let lossy = String::from_utf8_lossy(bytes).into_owned();
                self.vm.interner.intern(&lossy)
            }
        }
    }

    /// Source byte slice for a byte-offset location.
    pub(crate) fn slice(&self, bloc: (u32, u32)) -> &'a [u8] {
        self.src.get(bloc.0 as usize..bloc.1 as usize).unwrap_or(&[])
    }

    /// Char-offset range for a byte-offset location (the gem's `srange`).
    pub(crate) fn r(&self, bloc: (u32, u32)) -> R {
        R { b: self.off.c(bloc.0), e: self.off.c(bloc.1) }
    }

    /// 1-based line number of a byte offset (buffer first_line applied) —
    /// `Range#line` for `__LINE__` literals.
    pub(crate) fn line_of(&self, byte: u32) -> i64 {
        let idx = match self.line_begins.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        idx as i64 + self.first_line
    }

    /// Byte offset of the start of the line containing `byte`.
    pub(crate) fn line_start(&self, byte: u32) -> u32 {
        let idx = match self.line_begins.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        self.line_begins[idx]
    }
}

fn line_begins_of(src: &[u8]) -> Vec<u32> {
    let mut begins = vec![0u32];
    for (i, b) in src.iter().enumerate() {
        if *b == b'\n' && i + 1 < src.len() {
            begins.push((i + 1) as u32);
        }
    }
    begins
}

// ---------------------------------------------------------------------------
// Host fn
// ---------------------------------------------------------------------------

/// `__rubyrs_wqtrans_tokenize(buffer, source, options_blob, buffer_name,
/// first_line, version)` → `[ast, comments, tokens, diags]` or `nil` to
/// decline to the interpreted translation.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    rt.register_fn("__rubyrs_wqtrans_tokenize", |args| {
        let [buffer, Value::Str(source), Value::Str(opts), Value::Str(name), Value::Int(first_line), Value::Int(version)] = args else {
            return Err(Trap {
                err: RubyError::ArgumentError {
                    msg: "__rubyrs_wqtrans_tokenize(buffer, source, options, name, first_line, version)".to_string(),
                },
                backtrace: vec![],
            });
        };
        let ptr = crate::vm::current_vm_ptr();
        if ptr.is_null() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "prism_wq: CURRENT_VM_PTR null — called outside host-fn scope".to_string(),
                },
                backtrace: vec![],
            });
        }
        // SAFETY: installed by the dispatch site for this call's synchronous
        // duration (same pattern as json_native.rs / prism_native.rs).
        let vm = unsafe { &mut *ptr };
        let result = tokenize(
            vm,
            buffer.clone(),
            source,
            &opts.content.borrow().clone(),
            &name.content.borrow().clone(),
            *first_line,
            *version,
        );
        match result {
            Ok(v) => Ok(v),
            Err(d) => {
                if std::env::var_os("RUBYRS_WQTRANS_DEBUG").is_some() {
                    eprintln!("wqtrans decline: {}", d.0);
                }
                Ok(Value::Nil)
            }
        }
    });
}

fn tokenize(
    vm: &mut Vm,
    buffer: Value,
    source: &Rc<crate::value::RStr>,
    opts_blob: &[u8],
    buffer_name: &[u8],
    first_line: i64,
    version: i64,
) -> CRes<Value> {
    if !(version == 33 || version == 34 || version == 40 || version == 41) {
        return decline("unsupported parser version");
    }
    let src: Vec<u8> = source.content.borrow().clone();

    // Parse+lex via the linked prism C library, honoring the translation's
    // options blob (filepath/version/partial_script/encoding — built by the
    // gem's own dump_options port in prism_native_backend.rb).
    let blob = crate::prism_native::serialize_parse_lex(&src, Some(opts_blob));
    if blob.is_empty() {
        return decline("empty serialize blob");
    }

    let parse = match decode_parse_lex(&blob, &src, vm) {
        Some(p) => p,
        None => return decline("wire decode failed"),
    };

    // The parser gem's buffer is UTF-8 (ProcessedSource forces it) or binary;
    // with `encoding: false` prism parses in the buffer encoding, so the two
    // agree. Anything else means an exotic caller — decline.
    let buffer_enc = source.encoding.get();
    if parse.enc != buffer_enc {
        // The one benign mismatch: prism reports US-ASCII for pure-ASCII
        // sources tagged UTF-8 (and vice versa) — both sides slice bytes
        // identically then.
        let ascii_ok = src.is_ascii()
            && matches!(parse.enc, EncodingTag::Utf8 | EncodingTag::UsAscii | EncodingTag::Binary)
            && matches!(buffer_enc, EncodingTag::Utf8 | EncodingTag::UsAscii | EncodingTag::Binary);
        if !ascii_ok {
            return decline("encoding mismatch");
        }
    }

    let off = OffsetCache::build(&src);
    let line_begins = line_begins_of(&src);

    let mut ctx = Ctx {
        vm,
        src: &src,
        blob: &blob,
        off,
        line_begins,
        first_line,
        buffer_name: buffer_name.to_vec(),
        version,
        enc: buffer_enc,
        diags: Vec::new(),
        // Parser::VariablesStack starts with one open frame.
        pattern_vars: vec![Vec::new()],
        cpool_base: parse.cpool_base,
        cpool_len: parse.cpool_len,
    };

    // 1. unwrap(): prism errors then warnings, mapped to parser diagnostics.
    //    (Replayed by the hook BEFORE the AST is touched, so an error raising
    //    Parser::SyntaxError mid-replay matches the interpreted order.)
    for e in &parse.errors {
        let row = compiler::error_diagnostic(&mut ctx, e)?;
        ctx.diags.push(row);
    }
    for w in &parse.warnings {
        if let Some(row) = compiler::warning_diagnostic(&mut ctx, w)? {
            ctx.diags.push(row);
        }
    }

    // 2. build_ast — only when the parse succeeded (result.success? — no
    //    errors). With errors present the interpreted path raises during
    //    unwrap on rubyrs (all_errors_are_fatal since RUBY_ENGINE != "ruby"),
    //    so the AST is never consulted; ship ast=nil in that case.
    let ast: Option<Box<builder::WqNode>> = if parse.errors.is_empty() {
        compiler::visit_root(&mut ctx, &parse.root)?
    } else {
        None
    };

    // 3. build_tokens — the ported lexer over the prism token stream.
    let tokens = lexer::translate_tokens(&mut ctx, &parse)?;

    // 4. Materialize the whole result as Ruby objects.
    materialize::materialize(ctx, parse, ast, tokens, buffer)
}
