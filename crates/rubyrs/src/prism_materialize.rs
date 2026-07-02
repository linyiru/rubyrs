//! ADR 0036 Slice 2 — materialize the Prism node tree NATIVELY in Rust.
//!
//! Slice 1 (prism_native.rs) let RuboCop's `parser_prism` engine run by feeding the linked
//! prism C library's serialize-parse blob to the prism gem's INTERPRETED
//! `Prism::Serialize.load_parse(_lex)`. That deserializer dominates the per-file cost
//! (~26ms/~43ms on a 600-line file vs ~0.3ms for the C parse+serialize itself) — an
//! interpretation tax CRuby never pays because its C extension builds the node graph in C.
//!
//! This module walks the same wire blob in Rust and builds the SAME Ruby object graph
//! directly in the rubyrs heap: instances of the prism gem's own Ruby classes
//! (`Prism::ProgramNode`, `Prism::Location`, `Prism::ParseResult`, …) with the exact ivars
//! the gem's `Node#initialize` would have set — including the gem's packed-Integer lazy
//! location representation (`(start << 32) | length`, inflated to a `Prism::Location` on
//! first access by the gem's own readers). Behavioral fidelity target: indistinguishable
//! from `Serialize.load_parse` output with `freeze: false`.
//!
//! Decode fidelity is table-driven: `prism_node_specs.rs` is GENERATED from the prism gem's
//! own generated `serialize.rb` (wire field kinds/order) + `node.rb` (ivar names), so the
//! table cannot drift from the format by hand-porting mistakes. Version drift is handled at
//! run time: the blob header's MAJOR/MINOR/PATCH must equal the generated `WIRE_VERSION`,
//! and the Ruby backend additionally checks the LOADED gem's `Serialize::*_VERSION` before
//! selecting the native path.
//!
//! Decline-don't-crash: every unexpected shape (version mismatch, unknown encoding, missing
//! Prism class, truncated blob, bignum-without-feature) returns `None`, which the host fn
//! surfaces as `nil`; the Ruby backend then falls back to the interpreted
//! `Serialize.load_parse`, whose behavior (including error raising) is the spec.
//!
//! GC safety: `Heap::alloc` never collects (collection only happens at explicit `maybe_gc`
//! call sites, none of which are reached inside a host fn), so the intermediate children
//! held in Rust `Vec<Value>`s while their parents are being built cannot be swept. This is
//! the same discipline json_native.rs relies on. On decline, any half-built subgraph is
//! unrooted garbage and is reclaimed by the next normal collection.

use std::cell::Cell;
use std::rc::Rc;

use crate::heap::HeapObj;
use crate::intern::SymId;
use crate::prism_node_specs::{DIAGNOSTIC_TYPES, NODE_SPECS, TOKEN_TYPES, WIRE_VERSION};
use crate::value::{Class, EncodingTag, Instance, IvarTable, RStr, Value};
use crate::vm::Vm;

/// Wire field kinds, as they appear in the generated `NODE_SPECS` table. One variant per
/// distinct loader expression in the gem's generated `Serialize::Loader#load_node`.
#[derive(Copy, Clone, Debug)]
pub(crate) enum FieldKind {
    /// `load_node` — a required child node.
    Node,
    /// `load_optional_node` — 0x00 marker byte means nil, else the node (marker is the
    /// node's own type byte, re-consumed).
    OptNode,
    /// `Array.new(load_varuint) { load_node }` — a node list.
    NodeList,
    /// `load_constant` — 1-based constant-pool index, never 0.
    Constant,
    /// `load_optional_constant` — 0 means nil.
    OptConstant,
    /// `Array.new(load_varuint) { load_constant }` — a Symbol list.
    ConstantList,
    /// `load_string` — tagged byteslice-of-source or embedded bytes; frozen String.
    Str,
    /// `load_location` — with `freeze: false`, the packed Integer `(start << 32) | length`.
    Location,
    /// `load_optional_location` — 0x00 marker means nil, else a packed location.
    OptLocation,
    /// `load_varuint` used as a plain Integer field (e.g. `depth`, `maximum`).
    VarUint,
    /// `io.getbyte` — a single byte as Integer (NumberedParametersNode#maximum).
    UInt8,
    /// `load_integer` — sign byte + varuint count + 32-bit varuint chunks (IntegerNode).
    Integer,
    /// `load_double` — 8-byte native-endian double (FloatNode).
    Double,
}

/// Byte cursor over the serialize blob. All reads are bounds-checked and return `None` on
/// truncation — the caller declines to the interpreted path rather than panicking.
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
        let s = self.buf.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }

    /// LEB128 varuint, capped at u64 (the wire only carries u32-range values; the cap is
    /// pure defense).
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

    /// Zigzag-encoded signed varint (start_line).
    fn varsint(&mut self) -> Option<i64> {
        let n = self.varuint()?;
        Some(((n >> 1) as i64) ^ -((n & 1) as i64))
    }

    fn u32_native(&mut self) -> Option<u32> {
        let b = self.read(4)?;
        Some(u32::from_ne_bytes(b.try_into().ok()?))
    }

    /// `unpack1("D")` — native-endian double, written by the C serializer on this host.
    fn f64_native(&mut self) -> Option<f64> {
        let b = self.read(8)?;
        Some(f64::from_ne_bytes(b.try_into().ok()?))
    }
}

/// Interned ivar/method-support symbols shared by every object the materializer builds.
struct CommonSyms {
    at_source: SymId,
    at_node_id: SymId,
    at_location: SymId,
    at_flags: SymId,
    at_start_offset: SymId,
    at_length: SymId,
    at_leading_comments: SymId,
    at_trailing_comments: SymId,
    at_start_line: SymId,
    at_offsets: SymId,
    at_key_loc: SymId,
    at_value_loc: SymId,
    at_type: SymId,
    at_message: SymId,
    at_level: SymId,
    at_value: SymId,
    at_comments: SymId,
    at_magic_comments: SymId,
    at_data_loc: SymId,
    at_errors: SymId,
    at_warnings: SymId,
    syntax: SymId,
    argument: SymId,
    load: SymId,
    default: SymId,
    verbose: SymId,
}

impl CommonSyms {
    fn new(vm: &mut Vm) -> Self {
        let mut i = |s: &str| vm.interner.intern(s);
        Self {
            at_source: i("@source"),
            at_node_id: i("@node_id"),
            at_location: i("@location"),
            at_flags: i("@flags"),
            at_start_offset: i("@start_offset"),
            at_length: i("@length"),
            at_leading_comments: i("@leading_comments"),
            at_trailing_comments: i("@trailing_comments"),
            at_start_line: i("@start_line"),
            at_offsets: i("@offsets"),
            at_key_loc: i("@key_loc"),
            at_value_loc: i("@value_loc"),
            at_type: i("@type"),
            at_message: i("@message"),
            at_level: i("@level"),
            at_value: i("@value"),
            at_comments: i("@comments"),
            at_magic_comments: i("@magic_comments"),
            at_data_loc: i("@data_loc"),
            at_errors: i("@errors"),
            at_warnings: i("@warnings"),
            syntax: i("syntax"),
            argument: i("argument"),
            load: i("load"),
            default: i("default"),
            verbose: i("verbose"),
        }
    }
}

/// The materializer. Lives for one `Prism.parse`/`parse_lex` host-fn call.
struct Mat<'a, 'vm> {
    vm: &'vm mut Vm,
    r: Reader<'a>,
    /// Raw source bytes — target of constant-pool and `load_string` byteslices.
    input: &'a [u8],
    /// The parse encoding from the blob (set by `load_encoding`); tags every String/Symbol
    /// the loader creates.
    enc: EncodingTag,
    /// The `Prism::Source` (or `ASCIISource`) instance every Location points back to.
    source_val: Value,
    syms: CommonSyms,
    /// Per-node-type resolved class + interned field ivar names, filled on first use.
    node_classes: Vec<Option<Rc<Class>>>,
    node_field_syms: Vec<Option<Box<[SymId]>>>,
    /// `Prism::Location` class, resolved on the first Location object (comments, errors,
    /// tokens all build many of them — one lookup, not one per object).
    location_class: Option<Rc<Class>>,
    /// Constant pool: absolute blob offset of the pool + memoized Symbols.
    cpool_base: usize,
    cpool: Vec<Option<SymId>>,
}

/// Resolve `Prism::<name>` in the VM's qualified-name class table. The gem must already be
/// loaded (the backend `require`s it before any parse); a missing class declines.
fn prism_class(vm: &mut Vm, name: &str) -> Option<Rc<Class>> {
    let qualified = format!("Prism::{name}");
    let id = vm.interner.intern(&qualified);
    vm.classes.get(&id).cloned()
}

/// Allocate an instance of `class` with `ivars` — the native equivalent of `Klass.new(...)`
/// for the data-carrier classes whose `initialize` only assigns ivars (all prism result
/// classes are of this shape; verified against parse_result.rb / node.rb).
fn alloc_instance(vm: &mut Vm, class: Rc<Class>, ivars: IvarTable) -> Option<Value> {
    // Respect an embedder heap cap (read-only check; never collects).
    vm.check_alloc().ok()?;
    let id = vm.heap.alloc(HeapObj::Instance(Instance {
        class,
        ivars,
        singleton_class: None,
        frozen: Cell::new(false),
    }));
    Some(Value::Object(id))
}

fn alloc_array(vm: &mut Vm, elems: Vec<Value>) -> Option<Value> {
    vm.check_alloc().ok()?;
    Some(Value::Array(vm.heap.alloc(HeapObj::Array(elems.into()))))
}

/// A new unfrozen String tagged with `enc` — what `bytes.force_encoding(enc)` yields.
fn new_str(bytes: Vec<u8>, enc: EncodingTag, frozen: bool) -> Value {
    let rs = RStr::from_bytes(bytes);
    rs.encoding.set(enc);
    rs.frozen.set(frozen);
    Value::Str(Rc::new(rs))
}

impl<'a, 'vm> Mat<'a, 'vm> {
    /// `Prism::Source.for(input)` — pick `ASCIISource` vs `Source` and build the instance.
    /// Mirrors parse_result.rb including the binary-with-multibyte niche (which
    /// `force_encoding`s the string we were handed, exactly like the gem does).
    fn build_source(vm: &mut Vm, syms: &CommonSyms, input_str: &Rc<RStr>) -> Option<Value> {
        let ascii_only = input_str.content.borrow().is_ascii();
        let class_name = if ascii_only {
            "ASCIISource"
        } else if input_str.encoding.get() == EncodingTag::Binary {
            input_str.encoding.set(EncodingTag::Utf8);
            if std::str::from_utf8(&input_str.content.borrow()).is_ok() {
                "Source"
            } else {
                input_str.encoding.set(EncodingTag::Binary);
                "ASCIISource"
            }
        } else {
            "Source"
        };
        let class = prism_class(vm, class_name)?;
        let mut ivars = IvarTable::default();
        ivars.insert(syms.at_source, Value::Str(input_str.clone()));
        ivars.insert(syms.at_start_line, Value::Int(1));
        let offsets = alloc_array(vm, Vec::new())?;
        ivars.insert(syms.at_offsets, offsets);
        alloc_instance(vm, class, ivars)
    }

    /// `load_header` — magic + exact version pin + the location-fields byte.
    fn header(&mut self) -> Option<()> {
        if self.r.read(5)? != b"PRISM" {
            return None;
        }
        let (maj, min, pat) = (self.r.u8()?, self.r.u8()?, self.r.u8()?);
        if (maj, min, pat) != WIRE_VERSION {
            return None;
        }
        // 0 = location fields included (the only mode the gem accepts).
        if self.r.u8()? != 0 {
            return None;
        }
        Some(())
    }

    /// `load_encoding` — the encoding name row; resolves to rubyrs's tag. Unknown names
    /// (possible only with exotic magic comments and no `_encoding_full`) decline.
    fn encoding(&mut self) -> Option<EncodingTag> {
        let n = self.r.varuint()? as usize;
        let name = std::str::from_utf8(self.r.read(n)?).ok()?;
        Vm::encoding_tag_from_str(name)
    }

    /// A `Prism::Location` OBJECT (comments / errors / tokens use real objects even with
    /// `freeze: false`; only node fields use the packed form).
    fn loc_object(&mut self, start: u64, length: u64) -> Option<Value> {
        if self.location_class.is_none() {
            self.location_class = Some(prism_class(self.vm, "Location")?);
        }
        let class = self.location_class.clone()?;
        let mut ivars = IvarTable::default();
        ivars.insert(self.syms.at_source, self.source_val.clone());
        ivars.insert(self.syms.at_start_offset, Value::Int(start as i64));
        ivars.insert(self.syms.at_length, Value::Int(length as i64));
        ivars.insert(self.syms.at_leading_comments, Value::Nil);
        ivars.insert(self.syms.at_trailing_comments, Value::Nil);
        alloc_instance(self.vm, class, ivars)
    }

    fn load_location_object(&mut self) -> Option<Value> {
        let start = self.r.varuint()?;
        let length = self.r.varuint()?;
        self.loc_object(start, length)
    }

    /// Packed node-field location: `(start << 32) | length` as a Ruby Integer, the gem's
    /// own `freeze: false` representation (its lazy readers inflate on demand).
    fn load_location_packed(&mut self) -> Option<Value> {
        let start = self.r.varuint()?;
        let length = self.r.varuint()?;
        if start > u32::MAX as u64 || length > u32::MAX as u64 {
            return None;
        }
        Some(Value::Int(((start << 32) | length) as i64))
    }

    /// `load_comments` — InlineComment (0) / EmbDocComment (1) instances.
    fn comments(&mut self) -> Option<Value> {
        let n = self.r.varuint()? as usize;
        let inline_class = prism_class(self.vm, "InlineComment")?;
        let embdoc_class = prism_class(self.vm, "EmbDocComment")?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let kind = self.r.varuint()?;
            let loc = self.load_location_object()?;
            let class = match kind {
                0 => inline_class.clone(),
                1 => embdoc_class.clone(),
                _ => return None,
            };
            let mut ivars = IvarTable::default();
            ivars.insert(self.syms.at_location, loc);
            out.push(alloc_instance(self.vm, class, ivars)?);
        }
        alloc_array(self.vm, out)
    }

    fn magic_comments(&mut self) -> Option<Value> {
        let n = self.r.varuint()? as usize;
        let class = prism_class(self.vm, "MagicComment")?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let key_loc = self.load_location_object()?;
            let value_loc = self.load_location_object()?;
            let mut ivars = IvarTable::default();
            ivars.insert(self.syms.at_key_loc, key_loc);
            ivars.insert(self.syms.at_value_loc, value_loc);
            out.push(alloc_instance(self.vm, class.clone(), ivars)?);
        }
        alloc_array(self.vm, out)
    }

    fn optional_location_object(&mut self) -> Option<Value> {
        if self.r.u8()? == 0 {
            Some(Value::Nil)
        } else {
            self.load_location_object()
        }
    }

    /// `load_errors` / `load_warnings` — ParseError/ParseWarning instances. `levels` maps
    /// the wire level byte to its Symbol (position = byte value).
    fn diagnostics(&mut self, class_name: &str, levels: &[SymId]) -> Option<Value> {
        let n = self.r.varuint()? as usize;
        let class = prism_class(self.vm, class_name)?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let type_idx = self.r.varuint()? as usize;
            let type_name = *DIAGNOSTIC_TYPES.get(type_idx)?;
            let type_sym = Value::Sym(self.vm.interner.intern(type_name));
            // load_embedded_string(encoding) — frozen message.
            let len = self.r.varuint()? as usize;
            let msg = new_str(self.r.read(len)?.to_vec(), self.enc, true);
            let loc = self.load_location_object()?;
            let level = *levels.get(self.r.u8()? as usize)?;
            let mut ivars = IvarTable::default();
            ivars.insert(self.syms.at_type, type_sym);
            ivars.insert(self.syms.at_message, msg);
            ivars.insert(self.syms.at_location, loc);
            ivars.insert(self.syms.at_level, Value::Sym(level));
            out.push(alloc_instance(self.vm, class.clone(), ivars)?);
        }
        alloc_array(self.vm, out)
    }

    /// Constant-pool fetch (0-based index), memoized. Entries slice either the SOURCE
    /// (owned constants, bit 31 clear) or the BLOB (shared constants, bit 31 set).
    fn cpool_sym(&mut self, index: usize) -> Option<SymId> {
        if let Some(Some(id)) = self.cpool.get(index) {
            return Some(*id);
        }
        let off = self.cpool_base.checked_add(index.checked_mul(8)?)?;
        let row = self.r.buf.get(off..off + 8)?;
        let start = u32::from_ne_bytes(row[0..4].try_into().ok()?);
        let length = u32::from_ne_bytes(row[4..8].try_into().ok()?) as usize;
        let bytes = if start & (1 << 31) == 0 {
            self.input.get(start as usize..(start as usize).checked_add(length)?)?
        } else {
            let s = (start & ((1 << 31) - 1)) as usize;
            self.r.buf.get(s..s.checked_add(length)?)?
        };
        // The interpreted path is `byteslice.force_encoding(enc).to_sym`; rubyrs's
        // interner is str-keyed, so mirror String#to_sym's lossy view for non-UTF-8.
        let sym = match std::str::from_utf8(bytes) {
            Ok(s) => self.vm.interner.intern(s),
            Err(_) => {
                let lossy = String::from_utf8_lossy(bytes).into_owned();
                self.vm.interner.intern(&lossy)
            }
        };
        *self.cpool.get_mut(index)? = Some(sym);
        Some(sym)
    }

    /// `load_constant` — 1-based wire index.
    fn constant(&mut self) -> Option<Value> {
        let idx = self.r.varuint()? as usize;
        Some(Value::Sym(self.cpool_sym(idx.checked_sub(1)?)?))
    }

    fn optional_constant(&mut self) -> Option<Value> {
        let idx = self.r.varuint()? as usize;
        if idx == 0 {
            Some(Value::Nil)
        } else {
            Some(Value::Sym(self.cpool_sym(idx - 1)?))
        }
    }

    /// `load_string` — type 1 slices the source, type 2 embeds bytes; both frozen and
    /// tagged with the parse encoding.
    fn string(&mut self) -> Option<Value> {
        match self.r.u8()? {
            1 => {
                let start = self.r.varuint()? as usize;
                let length = self.r.varuint()? as usize;
                let bytes = self.input.get(start..start.checked_add(length)?)?;
                Some(new_str(bytes.to_vec(), self.enc, true))
            }
            2 => {
                let length = self.r.varuint()? as usize;
                Some(new_str(self.r.read(length)?.to_vec(), self.enc, true))
            }
            _ => None,
        }
    }

    /// `load_integer` — sign byte, chunk count, base-2^32 varuint chunks (LSB first).
    /// In-i64 values (all real-world literals) decode inline; anything wider goes through
    /// BigInt (demoted when it fits, rubyrs's canonical Integer form — mirroring
    /// `bigint_to_value` without its GC checkpoint) or declines without the feature.
    fn integer(&mut self) -> Option<Value> {
        let negative = self.r.u8()? != 0;
        let len = self.r.varuint()? as usize;
        let mut digits = Vec::with_capacity(len);
        for _ in 0..len {
            let chunk = self.r.varuint()?;
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
                return Some(Value::Int(n));
            }
        }
        #[cfg(feature = "bignum")]
        {
            let sign = if negative { num_bigint::Sign::Minus } else { num_bigint::Sign::Plus };
            let b = num_bigint::BigInt::from_slice(sign, &digits);
            if let Ok(n) = i64::try_from(&b) {
                return Some(Value::Int(n));
            }
            self.vm.check_alloc().ok()?;
            return Some(Value::BigInt(self.vm.heap.alloc(HeapObj::BigInt(b))));
        }
        #[allow(unreachable_code)]
        None
    }

    /// `load_node` — one node instance, recursively. The heart of the decoder: reads the
    /// generated spec for the wire type and lands each field in its named ivar.
    fn node(&mut self) -> Option<Value> {
        let ty = self.r.u8()? as usize;
        let spec = NODE_SPECS.get(ty.checked_sub(1)?)?;
        let node_id = self.r.varuint()?;
        let location = self.load_location_packed()?;
        if spec.skip_uint32 {
            // DefNode's serialized-locals length — the Ruby loader discards it too.
            self.r.read(4)?;
        }
        let flags = self.r.varuint()?;

        // Resolve + memoize this type's class and field ivar symbols on first use.
        if self.node_classes[ty - 1].is_none() {
            self.node_classes[ty - 1] = Some(prism_class(self.vm, spec.name)?);
            let syms: Box<[SymId]> = spec
                .fields
                .iter()
                .map(|(_, ivar)| self.vm.interner.intern(ivar))
                .collect();
            self.node_field_syms[ty - 1] = Some(syms);
        }
        let class = self.node_classes[ty - 1].clone()?;

        let mut ivars = IvarTable::default();
        ivars.insert(self.syms.at_source, self.source_val.clone());
        ivars.insert(self.syms.at_node_id, Value::Int(node_id as i64));
        ivars.insert(self.syms.at_location, location);
        ivars.insert(self.syms.at_flags, Value::Int(flags as i64));

        for fi in 0..spec.fields.len() {
            let kind = spec.fields[fi].0;
            let value = match kind {
                FieldKind::Node => self.node()?,
                FieldKind::OptNode => {
                    if *self.r.buf.get(self.r.pos)? == 0 {
                        self.r.pos += 1;
                        Value::Nil
                    } else {
                        self.node()?
                    }
                }
                FieldKind::NodeList => {
                    let n = self.r.varuint()? as usize;
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        items.push(self.node()?);
                    }
                    alloc_array(self.vm, items)?
                }
                FieldKind::Constant => self.constant()?,
                FieldKind::OptConstant => self.optional_constant()?,
                FieldKind::ConstantList => {
                    let n = self.r.varuint()? as usize;
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        items.push(self.constant()?);
                    }
                    alloc_array(self.vm, items)?
                }
                FieldKind::Str => self.string()?,
                FieldKind::Location => self.load_location_packed()?,
                FieldKind::OptLocation => {
                    if self.r.u8()? == 0 {
                        Value::Nil
                    } else {
                        self.load_location_packed()?
                    }
                }
                FieldKind::VarUint => Value::Int(self.r.varuint()? as i64),
                FieldKind::UInt8 => Value::Int(self.r.u8()? as i64),
                FieldKind::Integer => self.integer()?,
                FieldKind::Double => Value::Float(self.r.f64_native()?),
            };
            let sym = self.node_field_syms[ty - 1].as_ref()?[fi];
            ivars.insert(sym, value);
        }

        alloc_instance(self.vm, class, ivars)
    }

    /// `load_tokens` — `[[Prism::Token, lex_state], ...]`, terminator = token type 0.
    /// Token values are byteslices of the source; the caller re-tags them with the parse
    /// encoding once it is known (the loader's trailing `force_encoding` pass).
    fn tokens(&mut self) -> Option<Value> {
        let enc = self.enc;
        let token_class = prism_class(self.vm, "Token")?;
        let type_sym = self.vm.interner.intern("@type");
        let value_sym = self.vm.interner.intern("@value");
        let mut out = Vec::new();
        loop {
            let ty = self.r.varuint()? as usize;
            if ty == 0 {
                break;
            }
            let type_name = *TOKEN_TYPES.get(ty)?;
            let type_id = self.vm.interner.intern(type_name);
            let start = self.r.varuint()?;
            let length = self.r.varuint()?;
            let lex_state = self.r.varuint()?;
            let loc = self.loc_object(start, length)?;
            let slice = self
                .input
                .get(start as usize..(start as usize).checked_add(length as usize)?)?;
            let value = new_str(slice.to_vec(), enc, false);
            let mut ivars = IvarTable::default();
            ivars.insert(self.syms.at_source, self.source_val.clone());
            ivars.insert(type_sym, Value::Sym(type_id));
            ivars.insert(value_sym, value);
            ivars.insert(self.syms.at_location, loc);
            let token = alloc_instance(self.vm, token_class.clone(), ivars)?;
            let pair = alloc_array(self.vm, vec![token, Value::Int(lex_state as i64)])?;
            out.push(pair);
        }
        alloc_array(self.vm, out)
    }
}

/// The common (post-tokens) body shared by parse and parse_lex: header → encoding →
/// source line table → comments/diagnostics → constant pool → node tree. Returns the
/// pieces the result-object builders assemble.
struct LoadedParse {
    node: Value,
    comments: Value,
    magic_comments: Value,
    data_loc: Value,
    errors: Value,
    warnings: Value,
}

fn load_parse_body(m: &mut Mat) -> Option<LoadedParse> {
    m.header()?;
    let enc = m.encoding()?;
    m.enc = enc;

    let start_line = m.r.varsint()?;
    let offset_count = m.r.varuint()? as usize;
    let mut offsets = Vec::with_capacity(offset_count);
    for _ in 0..offset_count {
        offsets.push(Value::Int(m.r.varuint()? as i64));
    }

    // source.replace_start_line / replace_offsets.
    let offsets_val = alloc_array(m.vm, offsets)?;
    if let Value::Object(sid) = m.source_val {
        let inst = m.vm.heap.instance_mut(sid);
        inst.ivars.insert(m.syms.at_start_line, Value::Int(start_line));
        inst.ivars.insert(m.syms.at_offsets, offsets_val);
    }

    let comments = m.comments()?;
    let magic_comments = m.magic_comments()?;
    let data_loc = m.optional_location_object()?;
    let error_levels = [m.syms.syntax, m.syms.argument, m.syms.load];
    let errors = m.diagnostics("ParseError", &error_levels)?;
    let warning_levels = [m.syms.default, m.syms.verbose];
    let warnings = m.diagnostics("ParseWarning", &warning_levels)?;

    let cpool_base = m.r.u32_native()? as usize;
    let cpool_size = m.r.varuint()? as usize;
    // The pool rows must live inside the blob (soft structural validation, mirroring the
    // loader's trailing eof? check).
    cpool_base.checked_add(cpool_size.checked_mul(8)?)?.le(&m.r.buf.len()).then_some(())?;
    m.cpool_base = cpool_base;
    m.cpool = vec![None; cpool_size];

    let node = m.node()?;

    Some(LoadedParse { node, comments, magic_comments, data_loc, errors, warnings })
}

/// Assemble a `Prism::ParseResult` / `Prism::ParseLexResult` (`value` differs).
fn build_result(m: &mut Mat, class_name: &str, value: Value, body: LoadedParse) -> Option<Value> {
    let class = prism_class(m.vm, class_name)?;
    let mut ivars = IvarTable::default();
    ivars.insert(m.syms.at_value, value);
    ivars.insert(m.syms.at_comments, body.comments);
    ivars.insert(m.syms.at_magic_comments, body.magic_comments);
    ivars.insert(m.syms.at_data_loc, body.data_loc);
    ivars.insert(m.syms.at_errors, body.errors);
    ivars.insert(m.syms.at_warnings, body.warnings);
    ivars.insert(m.syms.at_source, m.source_val.clone());
    alloc_instance(m.vm, class, ivars)
}

fn new_mat<'a, 'vm>(vm: &'vm mut Vm, blob: &'a [u8], input: &'a [u8], source_val: Value, syms: CommonSyms) -> Mat<'a, 'vm> {
    Mat {
        vm,
        r: Reader { buf: blob, pos: 0 },
        input,
        enc: EncodingTag::Utf8,
        source_val,
        syms,
        node_classes: vec![None; NODE_SPECS.len()],
        node_field_syms: vec![None; NODE_SPECS.len()],
        location_class: None,
        cpool_base: 0,
        cpool: Vec::new(),
    }
}

/// Native `Serialize.load_parse(input, blob, false)`: returns the `Prism::ParseResult`, or
/// `None` to decline (caller falls back to the interpreted deserializer).
///
/// `source` is the caller's source String; per the gem it is `dup`ed first, so the caller's
/// string is never mutated here.
pub(crate) fn materialize_parse(vm: &mut Vm, source: &Rc<RStr>, blob: &[u8]) -> Option<Value> {
    let input_bytes: Vec<u8> = source.content.borrow().clone();
    // input = input.dup — same bytes + encoding tag, unfrozen.
    let dup = Rc::new(RStr::from_bytes(input_bytes.clone()));
    dup.encoding.set(source.encoding.get());

    let syms = CommonSyms::new(vm);
    let ascii_only = input_bytes.is_ascii();
    let source_val = Mat::build_source(vm, &syms, &dup)?;
    let mut m = new_mat(vm, blob, &input_bytes, source_val, syms);
    let body = load_parse_body(&mut m)?;

    // Serialize.load_parse tail (parse ONLY — parse_lex leaves the source string's
    // encoding alone): adopt the parse encoding, then the binary-but-valid-UTF-8 niche.
    let enc = m.enc;
    dup.encoding.set(enc);
    if !ascii_only && enc == EncodingTag::Binary {
        dup.encoding.set(EncodingTag::Utf8);
        if std::str::from_utf8(&dup.content.borrow()).is_err() {
            dup.encoding.set(EncodingTag::Binary);
        }
    }

    let node = body.node.clone();
    build_result(&mut m, "ParseResult", node, body)
}

/// Native `Serialize.load_parse_lex(input, blob, false)`: returns the
/// `Prism::ParseLexResult` (`value == [root_node, tokens]`), or `None` to decline.
///
/// Per the gem, parse_lex does NOT dup: the Source wraps the caller's string object and the
/// trailing encoding fixups apply to it.
pub(crate) fn materialize_parse_lex(vm: &mut Vm, source: &Rc<RStr>, blob: &[u8]) -> Option<Value> {
    let input_bytes: Vec<u8> = source.content.borrow().clone();
    let syms = CommonSyms::new(vm);
    let source_val = Mat::build_source(vm, &syms, source)?;
    let mut m = new_mat(vm, blob, &input_bytes, source_val, syms);

    // Tokens come FIRST on the parse_lex wire, before the encoding row is readable; the
    // loader force-encodes every token value after the body loads, so build them with a
    // placeholder tag and re-tag below.
    let tokens = m.tokens()?;
    let body = load_parse_body(&mut m)?;

    // token[0].value.force_encoding(encoding) — retro-tag every token value.
    let enc = m.enc;
    if let Value::Array(tid) = tokens {
        let pairs: Vec<Value> = m.vm.heap.array(tid).to_vec();
        for pair in pairs {
            if let Value::Array(pid) = pair
                && let Some(Value::Object(tok_id)) = m.vm.heap.array(pid).first().cloned()
            {
                let value_sym = m.vm.interner.intern("@value");
                if let Some(Value::Str(s)) = m.vm.heap.instance(tok_id).ivars.get(&value_sym) {
                    s.encoding.set(enc);
                }
            }
        }
    }

    let node = body.node.clone();
    let value = alloc_array(m.vm, vec![node, tokens])?;
    build_result(&mut m, "ParseLexResult", value, body)
}
