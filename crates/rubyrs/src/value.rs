use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::intern::{FxHashMap, SymId};

/// Heap-shared string body with a frozen flag. Holds raw bytes
/// (not Rust `String`) so that arbitrary byte sequences can
/// round-trip through Ruby — required for binary protocols
/// (msgpack, protobuf, etc.) where cext output isn't valid UTF-8.
///
/// `RefCell<Vec<u8>>` so aliases see mutations and a `Cell<bool>`
/// so `freeze` / `frozen?` round-trip without touching the
/// content borrow. Helper accessors below give string-shaped
/// views via `from_utf8_lossy` for code paths that need text;
/// the byte path is the cheap one (zero copy).
#[derive(Debug)]
/// The byte storage of an `RStr`, bundling the content cell with a
/// cached `ruby_hash` value so string Hash keys don't re-hash their
/// bytes on every probe (Jekyll's data hashes probe long string keys
/// constantly; FNV over a ~30-byte key was ~3× CRuby's per-lookup
/// cost).
///
/// INVALIDATION CONTRACT: every mutation acquires the buffer through
/// `borrow_mut()`, which clears the cache unconditionally
/// (invalidate-on-acquire — conservative for read-modify cycles that
/// end up not writing, but it makes the cache impossible to leave
/// stale: there is NO other way to get `&mut` at the bytes). The
/// cache stores only hashes of all-ASCII content, where `ruby_hash`
/// is encoding-tag-independent — so `force_encoding` & co. (which
/// flip `RStr.encoding` without touching content) never need to know
/// the cache exists. `0` is the "empty" sentinel; a computed hash of
/// 0 is remapped to 1 (FNV never produces 0 for non-empty input in
/// practice, and correctness only needs *some* stable non-zero
/// value).
pub struct StrCell {
    bytes: RefCell<Vec<u8>>,
    hash_cache: Cell<u64>,
    /// Cached ASCII-ness of the current content: -1 unknown, 0 not
    /// ASCII-only, 1 ASCII-only. `bytes.is_ascii()` is O(n), and
    /// `char_count` (String#length/#size) runs it on EVERY call — for
    /// a long ASCII string scanned in a loop (rack multipart's
    /// `StringScanner#eos?` → `@str.length` per part) that alone is
    /// O(n²). Caching the content-only ASCII flag makes `length` O(1)
    /// once computed; it's encoding-independent (ASCII content counts
    /// the same in any ASCII-compatible encoding) so a `force_encoding`
    /// never invalidates it — only a content write does.
    ascii_cache: Cell<i8>,
}

impl StrCell {
    #[inline]
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: RefCell::new(bytes),
            hash_cache: Cell::new(0),
            ascii_cache: Cell::new(-1),
        }
    }

    /// Read access — passthrough to the inner `RefCell`.
    #[inline]
    pub fn borrow(&self) -> std::cell::Ref<'_, Vec<u8>> {
        self.bytes.borrow()
    }

    /// Write access — clears the cached hash AND ASCII flag BEFORE
    /// handing out the guard (see the invalidation contract above).
    #[inline]
    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, Vec<u8>> {
        self.hash_cache.set(0);
        self.ascii_cache.set(-1);
        self.bytes.borrow_mut()
    }

    /// Is the content ASCII-only? Caches the O(n) scan; the cache is
    /// reset by `borrow_mut`.
    #[inline]
    pub(crate) fn is_ascii_cached(&self) -> bool {
        match self.ascii_cache.get() {
            1 => true,
            0 => false,
            _ => {
                let a = self.bytes.borrow().is_ascii();
                self.ascii_cache.set(if a { 1 } else { 0 });
                a
            }
        }
    }

    /// Cached `ruby_hash` for the current content, or 0 if not
    /// computed / not cacheable (non-ASCII content hashes are
    /// encoding-tag-dependent and stay uncached).
    #[inline]
    pub(crate) fn cached_hash(&self) -> u64 {
        self.hash_cache.get()
    }

    /// Store a freshly computed all-ASCII content hash.
    #[inline]
    pub(crate) fn set_cached_hash(&self, h: u64) {
        self.hash_cache.set(if h == 0 { 1 } else { h });
    }
}

#[derive(Debug)]
pub struct RStr {
    pub(crate) content: StrCell,
    pub(crate) frozen: Cell<bool>,
    /// ADR 0020 Phase E1: the encoding TAG (carried, not yet
    /// consumed — semantic enforcement of `==`/concat
    /// compatibility and the `force_encoding`/`encoding`
    /// reflection land in the E1 follow-up; this step only
    /// threads the field through every construction site so
    /// that change is a pure-semantics diff).
    pub(crate) encoding: Cell<EncodingTag>,
    /// `Some(c)` when this String is an instance of a user subclass of
    /// String (`class Password < String`). The string still IS a String
    /// (so String primitives — `replace`, `split`, `[]`, `==`, … —
    /// dispatch on it), but reports `c` as its class and consults `c`'s
    /// method chain for user overrides before the primitives. `None` for
    /// a plain `"..."` literal / `String.new`. The Array/Hash twin of
    /// `ArrayObj::class_tag`; the `Rc<Class>` is also rooted in
    /// `Vm.classes`, so it needs no extra GC marking.
    pub(crate) class_tag: std::cell::RefCell<Option<std::rc::Rc<Class>>>,
}

/// ADR 0020's Tier 1 encoding tag. `Other(u8)` indexes the Tier 2
/// `_encoding_full` registry (absent today — constructing it is
/// not yet possible; the variant exists so downstream match arms
/// are written exhaustively from day one).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EncodingTag {
    /// String literals, interpolation results, text file reads —
    /// anything the runtime knows (or assumes, per the current
    /// UTF-8-lossy contract) to be UTF-8.
    Utf8,
    /// `force_encoding("US-ASCII")` / Encoding::US_ASCII. The
    /// ASCII subset of UTF-8 — same byte semantics, narrower
    /// validity (`valid_encoding?` requires all bytes < 0x80).
    UsAscii,
    /// CRuby's ASCII-8BIT: cext binary input, pack/unpack
    /// buffers, digest output — opaque bytes, no codepoint
    /// semantics.
    Binary,
    /// A Tier 2 registry index (Latin-1, Shift_JIS, …).
    #[allow(dead_code)]
    Other(u8),
}

impl EncodingTag {
    /// CRuby's error-message display name (the BINARY dual-name;
    /// registry tags resolve through `encoding_full` when built).
    pub(crate) fn display(self) -> &'static str {
        match self {
            EncodingTag::Utf8 => "UTF-8",
            EncodingTag::UsAscii => "US-ASCII",
            EncodingTag::Binary => "BINARY (ASCII-8BIT)",
            #[cfg(feature = "_encoding_full")]
            EncodingTag::Other(idx) => {
                crate::encoding_full::name(idx).unwrap_or("OTHER")
            }
            #[cfg(not(feature = "_encoding_full"))]
            EncodingTag::Other(_) => "OTHER",
        }
    }
}

/// CRuby's `rb_enc_compatible` for the E1 tag set: same tag wins;
/// across tags, an ASCII-only side is compatible with anything and
/// the result takes the OTHER side's encoding (receiver wins when
/// both are ASCII-only); two non-ASCII sides with different tags
/// are incompatible (`None` → Encoding::CompatibilityError at the
/// call site). Byte slices come in because ascii-only-ness is a
/// content property, not a tag property.
pub(crate) fn enc_compat(
    a_tag: EncodingTag,
    a_bytes: &[u8],
    b_tag: EncodingTag,
    b_bytes: &[u8],
) -> Option<EncodingTag> {
    if a_tag == b_tag {
        return Some(a_tag);
    }
    let a_ascii = a_bytes.iter().all(|&x| x < 0x80);
    let b_ascii = b_bytes.iter().all(|&x| x < 0x80);
    match (a_ascii, b_ascii) {
        (true, true) => Some(a_tag),
        (true, false) => Some(b_tag),
        (false, true) => Some(a_tag),
        (false, false) => None,
    }
}

impl RStr {
    /// Construct from a Rust `String`. The bytes are consumed
    /// (cheap — no copy) and the `Vec<u8>` becomes the backing
    /// store; the UTF-8 invariant is preserved as long as the
    /// content isn't later overwritten with non-UTF-8 bytes by
    /// `borrow_mut()` callers (e.g. cext binary input).
    pub fn new(s: String) -> Self {
        Self {
            content: StrCell::new(s.into_bytes()),
            frozen: Cell::new(false),
            encoding: Cell::new(EncodingTag::Utf8),
            class_tag: std::cell::RefCell::new(None),
        }
    }

    /// Construct a US-ASCII-tagged string. CRuby builds the output of
    /// numeric `to_s`/`inspect` (`42.to_s`, `255.to_s(16)`,
    /// `1.5.to_s`, Bignum) as US-ASCII, not UTF-8 — the bytes are
    /// ASCII-only by construction. The caller must pass ASCII content.
    pub fn new_us_ascii(s: String) -> Self {
        Self {
            content: StrCell::new(s.into_bytes()),
            frozen: Cell::new(false),
            encoding: Cell::new(EncodingTag::UsAscii),
            class_tag: std::cell::RefCell::new(None),
        }
    }

    /// Construct from raw bytes. Tags UTF-8: the dominant
    /// callers cross TEXT that merely isn't validated (file
    /// reads, sliced strings); the byte-oriented producers that
    /// genuinely mean ASCII-8BIT use `from_bytes_binary`.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            content: StrCell::new(bytes),
            frozen: Cell::new(false),
            encoding: Cell::new(EncodingTag::Utf8),
            class_tag: std::cell::RefCell::new(None),
        }
    }

    /// Construct from raw bytes tagged ASCII-8BIT (CRuby's
    /// BINARY): cext string input (`rb_str_new` is binary by
    /// contract), msgpack frames, pack output, raw digests.
    pub fn from_bytes_binary(bytes: Vec<u8>) -> Self {
        Self {
            content: StrCell::new(bytes),
            frozen: Cell::new(false),
            encoding: Cell::new(EncodingTag::Binary),
            class_tag: std::cell::RefCell::new(None),
        }
    }

    /// Convenient string view. Lossy on invalid UTF-8 — replaces
    /// each invalid byte sequence with U+FFFD. Allocates only when
    /// the bytes aren't already valid UTF-8 (`Cow::Owned`); the
    /// happy path is zero-copy (`Cow::Borrowed`).
    pub fn with_str_lossy<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        let b = self.content.borrow();
        let cow = String::from_utf8_lossy(&b);
        f(&cow)
    }

    /// Owned `String` copy, lossy. Use for trait impls that need
    /// `String` ownership (Display, etc.).
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.content.borrow()).into_owned()
    }

    /// UTF-8 character count — what Ruby's `String#length` / `#size`
    /// returns. ASCII-only buffers short-circuit on `is_ascii()`
    /// (every byte is one char); non-ASCII falls back to a chars
    /// walk over the lossy view, where invalid byte sequences each
    /// count as one U+FFFD (matches CRuby's "length on a UTF-8
    /// String" semantic). The shortcut means the canonical builtin
    /// path and dispatch's primitive fast-path stay in lock-step
    /// without one of them silently winning a perf round.
    ///
    /// ASCII-8BIT (BINARY) strings count every byte as one char
    /// (`length == bytesize`) regardless of whether the bytes form
    /// valid UTF-8 — CRuby semantics. This matters for binary payloads
    /// (e.g. multipart file uploads read through StringIO): treating a
    /// binary buffer as UTF-8 would under-count, desyncing byte-offset
    /// arithmetic that consumers do with `length`.
    pub fn char_count(&self) -> usize {
        let bytes = self.content.borrow();
        if matches!(self.encoding.get(), EncodingTag::Binary) {
            return bytes.len();
        }
        if self.content.is_ascii_cached() {
            bytes.len()
        } else {
            String::from_utf8_lossy(&bytes).chars().count()
        }
    }
}

impl std::ops::Deref for RStr {
    type Target = StrCell;
    fn deref(&self) -> &Self::Target { &self.content }
}


/// Display form for a Class value: the effective name when one
/// exists, otherwise CRuby's `#<Class:0xADDR>` placeholder — with
/// the ADR 0017 twist that the hex digits are the deterministic
/// `anon_serial` creation counter (stamped by Class.new/Module.new),
/// not a raw address. A never-stamped anonymous class (internal
/// construction paths) renders without the id.
pub(crate) fn class_display_name(c: &std::rc::Rc<Class>) -> String {
    if let Some(n) = c.effective_name() {
        return n;
    }
    let kind = if c.is_module { "Module" } else { "Class" };
    let serial = c.anon_serial.get();
    if serial == 0 {
        format!("#<{kind}>")
    } else {
        format!("#<{kind}:0x{:012x}>", serial)
    }
}

/// Method visibility. Default is `Public`; `private` / `protected`
/// inside a class body changes the mode for subsequent `def`s and
/// `private :sym` retroactively flips already-defined methods.
/// `Protected` is enforced identically to `Public` for now (same-
/// instance / same-class check is more invasive than the subset
/// warrants) and `Private` blocks any call with an explicit
/// receiver — `obj.priv_method` raises NoMethodError; `priv_method`
/// (no receiver, implicit self) is allowed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

// ---------- Values ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjId(pub(crate) u32);

#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    /// Arbitrary-precision integer. CRuby unified Fixnum + Bignum
    /// into Integer at 2.4; we model it as an i64 fast path plus
    /// a heap-allocated `num_bigint::BigInt` slow path. Promotion
    /// is implicit: any arithmetic op that would overflow i64
    /// promotes to BigInt; any BigInt result that fits back in
    /// i64 demotes on creation (so `Fixnum == Bignum` equality
    /// stays the natural `Int(5) == Int(5)` case for the common
    /// post-overflow shrink).
    ///
    /// Cfg-gated on the `bignum` feature (ADR 0018's BigInt
    /// placement decision — Tier-1 semantics, Tier-1 implementation
    /// dep). With `--no-default-features`, the variant disappears
    /// and arithmetic falls back to i64 two's-complement `wrapping_*`
    /// (wraps on overflow rather than promoting).
    #[cfg(feature = "bignum")]
    BigInt(ObjId),
    /// 64-bit float. Mixed arithmetic with Int promotes the Int
    /// (CRuby's "Float wins on mix" rule). Equality across the
    /// numeric types coerces too — `5 == 5.0` is `true`.
    Float(f64),
    /// Tier-2 rational number (Phase C.1). Heap-stored
    /// `RationalRepr { num: i64, den: i64 }` always in lowest terms
    /// with `den > 0`. Constructed via `Kernel#Rational(n, d)` which
    /// gcd-normalizes + sign-normalizes at the constructor boundary,
    /// so every live `Value::Rational` is canonical. Cross-type
    /// equality (Rational == Integer / Float) flows through the
    /// Numeric#coerce protocol; in-band arithmetic + comparison is
    /// added in Phase C.2.
    Rational(ObjId),
    /// Mutable, optionally-frozen string. `Rc<RStr>` shares one
    /// content + frozen-flag pair across every Value clone — so
    /// `s[i] = x` and `s.freeze` both have global-to-aliases
    /// effect, matching CRuby's mutable-object semantics. `RStr`
    /// derefs to its inner `RefCell<String>`, so existing
    /// `.borrow()` / `.borrow_mut()` sites keep working unchanged.
    Str(Rc<RStr>),
    Sym(SymId),
    Bool(bool),
    Nil,
    Class(Rc<Class>),
    Object(ObjId),
    Array(ObjId),
    Hash(ObjId),
    Range(ObjId),
    /// `Proc`-flavoured block value. Heap-managed since P2-13 —
    /// before that this was `Block(Rc<BlockHandle>)`, which formed
    /// an Rc cycle whenever a block's `captured` slots held the
    /// block itself (e.g. `p = proc { p }` patterns common in
    /// callback DSLs). Now the BlockHandle lives in a heap slot
    /// and is mark-swept like Array/Hash/Range.
    Block(ObjId),
    /// `/pattern/` literal. The compiled `CompiledRegex` is
    /// shared via Rc — both inner engines are immutable so
    /// there's no aliasing risk.
    ///
    /// `CompiledRegex` wraps either `regex::Regex` (linear-
    /// time, the preferred backend — used for the vast
    /// majority of Ruby patterns) or `fancy_regex::Regex`
    /// (backtracking NFA, used as a fallback when the linear
    /// engine rejects the pattern as a syntax error — typically
    /// Ruby's lookaround `(?=...)` / `(?!...)` constructs).
    /// fancy-regex itself delegates simple subpatterns to the
    /// linear engine, so the cold path is mostly still linear;
    /// only the fancy-only features run through the
    /// backtracker. ReDoS hardening is preserved for native-
    /// path patterns; fancy patterns carry the standard
    /// backtracking risk. (TRY_RUNS pass-13 layer #17.)
    ///
    /// Remaining dialect gaps vs Onigmo (CRuby's engine) —
    /// possessive quantifiers, some `\k<name>` backref forms —
    /// are documented in SUBSET.md.
    ///
    /// Cfg-gated on the `regex` feature (per ADR 0017 Rule 3
    /// regex is a Tier-2 feature). With `--no-default-features`
    /// the variant disappears; AST translation rejects `/.../`
    /// literals with a clear trap; every dispatch arm matching
    /// `Value::Regex(_)` cfg's out.
    ///
    /// The inner `CompiledRegex` is an enum over the linear-
    /// time `regex` engine (preferred) and the `fancy-regex`
    /// backtracking engine (fallback for lookaround / backref
    /// patterns the linear engine rejects). See
    /// `regex_engine::compile`. (TRY_RUNS pass-13 layer #17.)
    #[cfg(feature = "regex")]
    Regex(std::rc::Rc<crate::regex_engine::CompiledRegex>),
    /// `Object#method(:foo)` result — a captured (receiver,
    /// method-name) pair. Heap-managed so the GC walks the
    /// inner receiver (it can hold any other Value, including
    /// other heap references). `.call(args)` / `.()` / `[args]`
    /// dispatches the captured method on the captured receiver.
    BoundMethod(ObjId),
    /// `Method#unbind` result — a captured (class, method-name)
    /// pair with no receiver. `.bind(obj)` produces a fresh
    /// BoundMethod, provided `obj.is_a?(class)`. Heap slot is
    /// used for parity with BoundMethod; the inner `Rc<Class>`
    /// is not heap-managed so no GC walk is needed.
    UnboundMethod(ObjId),
    /// `Method#curry` / `Proc#curry` result. Carries the
    /// underlying callable, args gathered so far, and the target
    /// arity. Each `.call` either invokes the underlying (once
    /// gathered.len() >= target_arity) or returns a new
    /// CurriedProc with the new args appended. `class_of`
    /// reports it as `Proc` to match CRuby.
    CurriedProc(ObjId),
}

#[derive(Debug)]
pub struct BlockHandle {
    pub(crate) proto_idx: usize,
    /// Shared with the frame the block executes in: when
    /// `Vm::invoke_block` pushes a frame for this block, the frame
    /// borrows the SAME `Rc<RefCell<Vec<Value>>>`, so writes to
    /// outer-frame variables inside the block are visible to
    /// subsequent invocations. The Rc here is shared frame-wise,
    /// not as a back-edge for ownership of the BlockHandle itself
    /// — that's the heap slot's job.
    pub(crate) captured: Rc<RefCell<Vec<Value>>>,
    pub(crate) self_val: Value,
    /// The lexical class for `@@cvar` resolution — the class whose body
    /// or method lexically encloses this block at creation time. CRuby
    /// resolves class variables through the lexical cref, NOT `self`, so
    /// a block run with a different self (`instance_eval` / `class_eval`)
    /// must still see the cvars of where it was written. Captured at
    /// `Op::CreateBlock` and threaded onto the block's frame; `None` for
    /// blocks created at the top level (cvars fall back to
    /// `Vm.toplevel_cvars`). See `Vm::surrounding_class`.
    pub(crate) lexical_cvar_class: Option<std::rc::Rc<Class>>,
    pub(crate) param_start: u16,
    pub(crate) n_params: u16,
    /// `Some(slot)` when the block declares a `*rest` parameter.
    /// `slot` is the local-slot index where the rest collector
    /// lives. Filled by `invoke_block` with a fresh Array of any
    /// args past the last required slot. `None` means no rest —
    /// overflow args are silently dropped (CRuby behaviour for
    /// blocks).
    pub(crate) rest_slot: Option<u16>,
    /// `Some(slot)` when the block declares a `|**opts|`
    /// keyword-rest param. `invoke_block` binds the trailing
    /// kwargs Hash (or a fresh `{}`) into this slot. `None` means
    /// no `**opts` — a trailing Hash arg stays a positional.
    pub(crate) kw_rest_slot: Option<u16>,
    /// `true` when `captured` is the locals of a METHOD / class-body /
    /// toplevel frame (a real outer scope), `false` when it's another
    /// block's locals (this block was created inside an enclosing
    /// block). The share-direct block-locals fast path
    /// (`block_frame_locals`) keys on this: sharing is only sound when
    /// outer-scope writes land on a genuine method scope — if
    /// `captured` is an enclosing block's per-invocation COPY, a direct
    /// share would skip that copy's write-back chain and lose the
    /// propagation to the grandparent (the `[[1,2]].each { |p| p.each
    /// { |n| total += n } }` case). Set at `Op::CreateBlock` from
    /// whether the creating frame `is_block`.
    pub(crate) captured_is_method_scope: bool,
    /// The block that `yield` inside this block must invoke — i.e. the
    /// block passed to the METHOD that lexically encloses this block at
    /// creation time. Captured at `Op::CreateBlock`: when the creating
    /// frame is a method/class-body/toplevel its `block_arg`; when the
    /// creating frame is itself a block, that block frame's own
    /// `captured_yield_block` (so the binding propagates through nested
    /// blocks). `Op::Yield` falls back to this when the lexical owner
    /// method is no longer on the stack — the ESCAPED-closure case
    /// (`def m(&blk); ->(){ yield }; end` returned then called later),
    /// where the live-frame walk (`lexical_owner_of_top`) finds no
    /// method frame to read `block_arg` from. `None` when no enclosing
    /// method had a block. GC: rooted via the heap block walk alongside
    /// `captured` / `self_val`.
    pub(crate) captured_yield_block: Option<ObjId>,
    /// `true` when this block is a LAMBDA (`-> { }`, `lambda { }`, or a
    /// `Method`/`Symbol` coerced via `#to_proc`), `false` for an
    /// ordinary proc/block. Backs `Proc#lambda?`. NOTE: this is purely
    /// the introspection bit — rubyrs does NOT (yet) enforce the
    /// lambda-vs-proc behavioural differences (strict arity, `return`
    /// scope), a documented subset gap.
    pub(crate) is_lambda: bool,
}

#[derive(Debug)]
pub struct Class {
    /// Class / Module identity name. Set once at first creation
    /// in `Op::DefClass` — the qualified form (`"Foo::Bar"`) when
    /// defined inside a `module` / `class` body, or the bare form
    /// (`"Bar"`) at the top level. Re-opens within the same scope
    /// hit the same class-table slot, so they never re-stamp the
    /// name; re-opens in a different scope create a separate Class
    /// (key-by-qualified-name landed in Step 1 of the #224
    /// refactor), each with its own immutable name. No `RefCell`
    /// — the field is effectively `set-once` for the lifetime of
    /// the `Class`.
    pub(crate) name: String,
    /// `true` when this Class shell models a `module X; end`
    /// declaration (or a stdlib-stub installed as a Module-
    /// shaped name like `URI` / `JSON`). Drives `Class#is_a?`
    /// (Module-shape returns false for `is_a?(Class)`, true
    /// for `is_a?(Module)`) and `class_of` (returns "Module"
    /// vs "Class"). Tier 1 still models both with the same
    /// underlying `Class` struct — the flag is the only
    /// runtime distinction. `class X; end` and Class-shaped
    /// stubs (`Logger`) keep this `false`.
    pub(crate) is_module: bool,
    /// Class-level instance variables — the `@foo = ...` slots
    /// that live ON the Class object itself (CRuby calls these
    /// "class instance variables" to distinguish from `@@foo`
    /// class-shared variables which we don't model). Written by
    /// `@foo = expr` in a class body OR inside a singleton/class
    /// method where `self` is `Value::Class(...)`, read by `@foo`
    /// in the same contexts. Routes through the same `Op::LoadIvar`
    /// / `Op::StoreIvar` handlers that work on `Value::Object`;
    /// the handler picks the table based on receiver type.
    /// Inheritance: NOT inherited — each class has its own slot,
    /// matching CRuby. Use cases: `module Tilt; @default = ...;
    /// class << self; attr_accessor :default; end; end` round-
    /// trips, `Foo.instance_variable_set(...)` semantics later.
    pub(crate) ivars: RefCell<FxHashMap<SymId, Value>>,
    pub(crate) methods: RefCell<FxHashMap<SymId, Rc<Method>>>,
    /// Per-class singleton-method table — `def self.foo; ...; end`
    /// inside a class body installs `foo` here. Dispatched against
    /// `Value::Class(c)` receivers in `do_call`. Parallel to
    /// `cext_class_methods` (which holds C-ext-installed singletons
    /// keyed by class joined name); this one holds user-Ruby
    /// singletons keyed by interned method SymId on the Class
    /// itself, so it survives class re-opening naturally and
    /// doesn't need a separate generation counter.
    pub(crate) singleton_methods: RefCell<FxHashMap<SymId, Rc<Method>>>,
    /// Parent class for method lookup. `None` only for the implicit root
    /// (Object); every user-defined class has a superclass (defaulting to
    /// Object if not specified).
    pub(crate) superclass: RefCell<Option<Rc<Class>>>,
    /// Modules included into this class via `include Mod`. Stored in
    /// reverse-include order (last-included first), matching CRuby's
    /// "most recently included wins" lookup sequence. Method dispatch
    /// in `lookup_method_uncached` walks them after the class's own
    /// methods but before the superclass chain. `Class#ancestors`
    /// renders them between the class itself and its superclass.
    pub(crate) includes: RefCell<Vec<Rc<Class>>>,
    /// Modules prepended via `prepend Mod`. CRuby semantics:
    /// dispatch walks the prepend chain BEFORE the class's own
    /// methods (the opposite of include). `Class#ancestors`
    /// renders them ABOVE the class itself. Reverse-prepend order
    /// (last-prepended first) mirrors `includes`. Walked by
    /// `lookup_method_uncached` and `class_is_a` so `is_a?(M)`
    /// returns true for prepended modules too.
    pub(crate) prepends: RefCell<Vec<Rc<Class>>>,
    /// Modules prepended onto THIS class's singleton class —
    /// `class << X; prepend Mod; end`. CRuby installs them on
    /// X's eigenclass; we approximate by tracking the chain
    /// here next to `singleton_methods`. Dispatched against
    /// `Value::Class(c)` receivers in
    /// `lookup_class_singleton_method`, walked before the
    /// class's own `singleton_methods` at each superclass level.
    /// Same reverse-order convention as `prepends`.
    ///
    /// Motivating case: tilt.rb's `finalize!` does
    /// `class << self; prepend(Module.new { def lazy_map(*); ...; end }); end`
    /// to install an "after-freeze" guard layer in front of the
    /// class's own `register` / `lazy_map`.
    pub(crate) singleton_prepends: RefCell<Vec<Rc<Class>>>,
    /// Modules extended into THIS Class's singleton class —
    /// `Klass.extend Mod`. CRuby treats this as
    /// `class << Klass; include Mod; end`: Mod's instance
    /// methods become class-level methods of Klass. The
    /// dispatch arm walks this chain in
    /// `lookup_class_singleton_method` after the class's own
    /// `singleton_methods` and before the superclass step,
    /// matching CRuby's metaclass ancestor walk
    /// (Klass.singleton_class → extended modules → superclass.
    /// singleton_class). Reverse-include order
    /// (last-extended first) mirrors `includes` / `prepends`.
    ///
    /// Motivating case: sinatra-contrib's MultiRoute,
    /// Sinatra::Cors, etc. — `register Sinatra::MultiRoute`
    /// extends the app class with the MultiRoute module so
    /// `MyApp.get(*paths, &block)` resolves to MultiRoute's
    /// override (which calls `super` to reach Sinatra::Base's
    /// `get`). Before this field existed, `Klass.extend(M)`
    /// silently pushed M into `Klass.includes` instead,
    /// inverting the dispatch (instance methods got M's
    /// surface; class methods didn't).
    pub(crate) singleton_includes: RefCell<Vec<Rc<Class>>>,
    /// Lazy eigenclass shell returned by `Class#singleton_class`.
    /// `None` until the first call; subsequent calls return the
    /// same `Rc` so identity comparisons (`A.singleton_class.equal?(A.singleton_class)`)
    /// hold. The shell's `singleton_target` weak-points back at
    /// this Class so the 3 method-install paths
    /// (`Op::DefMethod` / `Op::DefMethodBlock` / runtime
    /// `Module#define_method`) can redirect installs from the
    /// shell's `methods` table to this Class's
    /// `singleton_methods`. That redirect is the whole point —
    /// `cls.singleton_class.class_eval { def foo; …; end }` and
    /// `cls.singleton_class.class_eval { define_method(:foo) { … } }`
    /// install `:foo` so `cls.foo` dispatches via the existing
    /// singleton-method lookup. Sinatra's `define_singleton`
    /// idiom (base.rb:1735) and the `set` getter installer
    /// (base.rb:1349-ish) both rely on this. Layer #23 of the
    /// TRY_RUNS pass series.
    pub(crate) singleton_view: RefCell<Option<Rc<Class>>>,
    /// When this Class is itself the eigenclass-shell returned
    /// by some other Class's `singleton_class`, this weak ref
    /// points back at the real Class so method installs into
    /// `self.methods` can be redirected to
    /// `singleton_target.singleton_methods`. `None` on every
    /// "real" Class. See `singleton_view` above.
    pub(crate) singleton_target: RefCell<Option<std::rc::Weak<Class>>>,
    /// Class variables (`@@foo`) defined on this class. Tier 1
    /// simplification: stored directly on the class (no
    /// hierarchy walk on read/write), so subclass `@@foo` and
    /// parent `@@foo` are independent rather than aliased.
    /// CRuby's read/write-walk-up-the-chain semantics would
    /// share `@@foo` across descendants; we don't model that
    /// because mainstream uses (Sinatra `@@eats_errors`,
    /// dry-struct caches, etc.) keep class vars on a single
    /// class anyway. Documented divergence — recorded in
    /// SUBSET.md when this lands.
    /// Names removed via `undef_method` / the `undef` keyword.
    /// A tombstone TERMINATES lookup for that name at this class
    /// (the walk must not continue to ancestors) and suppresses the
    /// builtin/universal arms — dispatch goes straight to
    /// method_missing, CRuby's undef semantics. Kept as a separate
    /// set (not a sentinel Method) so the `methods` RefCell's value
    /// type stays untouched. The Vm-level `any_undefs` flag gates
    /// every dispatch-side check, so programs that never undef pay
    /// one bool test.
    pub(crate) undefed: RefCell<crate::intern::FxHashSet<crate::intern::SymId>>,
    /// Deterministic serial for ANONYMOUS classes' display form —
    /// CRuby renders `#<Class:0xADDR>`; ADR 0017 keeps raw
    /// addresses out of Tier 1, so `Class.new`/`Module.new` stamp a
    /// per-VM creation counter here instead (same digits-shape, so
    /// consumers that normalize `0x[hex]+` — minitest's mu_pp
    /// comparisons — behave identically; run-to-run deterministic).
    /// 0 = never stamped (named classes; display omits the id).
    pub(crate) anon_serial: std::cell::Cell<u32>,
    pub(crate) class_vars: RefCell<FxHashMap<SymId, Value>>,
    /// Per-class constant storage for ANONYMOUS classes
    /// (cls.name == ""). Named classes still keep their
    /// constants in the Vm-level `self.constants` HashMap
    /// keyed by qualified name (`Foo::Bar::BAZ`) — that's
    /// the back-compat path for nested class bodies. Anon
    /// classes can't use the qualified-name scheme because
    /// `format!("{}::{}", "", "BAZ")` collapses to `"::BAZ"`
    /// which collides with the toplevel `BAZ` constant key
    /// (or, worse, `"BAZ"` itself if we strip the leading
    /// `::`). Routing anon-class `const_set` through this
    /// per-class table keeps each anon class's constants
    /// scoped to that class instance. `resolve_const_path`
    /// consults this table FIRST when the starting scope is
    /// anon, before falling through to the
    /// inheritance-chain walk.
    pub(crate) consts: RefCell<FxHashMap<SymId, Value>>,
    /// CRuby names an anonymous class/module on its FIRST
    /// constant assignment: `C = Class.new` makes `C.name == "C"`,
    /// `Foo::Bar = Class.new` makes the name `"Foo::Bar"`. The
    /// structural `name` field above is immutable (set once at
    /// `Op::DefClass`), so anon classes minted by `Class.new` /
    /// `Module.new` keep `name == ""` for their lifetime. This
    /// interior-mutable slot records the name STAMPED on first
    /// const-assignment (`Op::StoreConst` / `const_set`) without
    /// reconstructing the `Rc<Class>`. `Module#name` / `#to_s` /
    /// `#inspect` consult it when `name` is empty, and
    /// `resolve_const_path` uses it as the continuation scope-name
    /// so a deep chain (`C::Inner::Leaf`) resolves through a
    /// promoted anon class. `None` until first const-assignment;
    /// once `Some`, the matching qualified entries are also mirrored
    /// into the Vm-level `classes` / `constants` maps so the global
    /// qualified-key read paths find them. Singleton-class shells
    /// are never stamped here (they report `nil` for `name`).
    pub(crate) assigned_name: RefCell<Option<String>>,
    /// For a module VALUE that is an instance of a user-defined
    /// `Module` subclass (`class Tagged < Module; end; Tagged.new`):
    /// the actual class the module is an instance of. CRuby lets you
    /// subclass `Module`/`Class`; an instance of such a subclass IS a
    /// module/class (own `is_module`/method-table machinery) but its
    /// `.class` is the subclass, and method calls on it resolve the
    /// subclass's instance methods (dry-core's `Deprecations::Tagged`
    /// defines `extended`/`deprecation_tag` this way). `None` for an
    /// ordinary module/class (whose class is `Module`/`Class`). Set
    /// once at allocation (`Tagged.new`); read by `class_of` and the
    /// Class-receiver dispatch path.
    pub(crate) class_tag: Option<Rc<Class>>,
    /// L3-F: optional cext-side allocator. When `Klass.new(args)` is
    /// dispatched and this is `Some(fn)`, the host calls `fn(klass)`
    /// to produce the instance handle (typically a
    /// `TypedData_Wrap_Struct`-wrapped Object) instead of allocating
    /// a bare `Instance`. `initialize` is then called on the
    /// returned handle. Set only via cext-side `rb_define_alloc_func`
    /// drained by `Vm::cext_require`.
    ///
    /// With the `cext` feature off this field disappears entirely
    /// and the dispatcher's `do_call` Klass.new path is split by
    /// `#[cfg(feature = "cext")]` into a cext arm (this if/else)
    /// and a non-cext arm (default Instance allocation only). No
    /// `Option<()>` sentinel and no `unreachable!()` site in the
    /// non-cext build.
    #[cfg(feature = "cext")]
    pub(crate) cext_alloc_func: Cell<Option<rubyrs_cext::OpaqueFn>>,
}

impl Class {
    /// Install `m` under `name` into the appropriate method table.
    /// When `self` is an eigenclass-shell (built lazily by
    /// `Class#singleton_class`), redirect the install into the
    /// underlying real class's `singleton_methods` table so the
    /// real class's `Foo.method_name` dispatch finds it. Otherwise
    /// insert into the regular instance-methods table. Used by
    /// every `def` / `define_method` install path so all three
    /// behave consistently inside
    /// `cls.singleton_class.class_eval { … }`. Layer #23.
    pub(crate) fn install_method(self: &Rc<Self>, name: SymId, m: Rc<Method>) {
        if let Some(target) = self
            .singleton_target
            .borrow()
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
        {
            target.singleton_methods.borrow_mut().insert(name, m);
        } else {
            self.methods.borrow_mut().insert(name, m);
        }
    }

    /// Field-by-field shallow copy: fresh `RefCell` tables whose
    /// entries are `Rc`-shared with `self` (method bodies, includes,
    /// consts stay shared, but the maps themselves are independent so
    /// inserting into the copy doesn't mutate the original). Used by
    /// `Object#clone` / `Hash#clone` to give the copy its OWN
    /// singleton class — CRuby's `clone` copies the singleton class
    /// (and thus its singleton methods), where `dup` drops it.
    pub(crate) fn shallow_copy(&self) -> Class {
        use std::cell::{Cell, RefCell};
        Class {
            name: self.name.clone(),
            is_module: self.is_module,
            ivars: RefCell::new(self.ivars.borrow().clone()),
            methods: RefCell::new(self.methods.borrow().clone()),
            singleton_methods: RefCell::new(self.singleton_methods.borrow().clone()),
            superclass: RefCell::new(self.superclass.borrow().clone()),
            includes: RefCell::new(self.includes.borrow().clone()),
            prepends: RefCell::new(self.prepends.borrow().clone()),
            singleton_prepends: RefCell::new(self.singleton_prepends.borrow().clone()),
            singleton_includes: RefCell::new(self.singleton_includes.borrow().clone()),
            singleton_view: RefCell::new(self.singleton_view.borrow().clone()),
            singleton_target: RefCell::new(self.singleton_target.borrow().clone()),
            undefed: RefCell::new(self.undefed.borrow().clone()),
            anon_serial: Cell::new(self.anon_serial.get()),
            class_vars: RefCell::new(self.class_vars.borrow().clone()),
            consts: RefCell::new(self.consts.borrow().clone()),
            assigned_name: RefCell::new(self.assigned_name.borrow().clone()),
            class_tag: self.class_tag.clone(),
            #[cfg(feature = "cext")]
            cext_alloc_func: Cell::new(self.cext_alloc_func.get()),
        }
    }

    /// Lazily get-or-create this Class/Module's eigenclass shell —
    /// the `Rc<Class>` returned by `Class#singleton_class` and used
    /// as the `self` of a real `class << SomeClass; ...; end` body.
    /// The shell carries `singleton_target = Some(Weak(self))`, so
    /// `install_method` / the alias / visibility / include paths
    /// redirect installs into `self.singleton_methods` rather than
    /// the shell's own (empty) `methods` table. Caching on
    /// `singleton_view` keeps identity stable
    /// (`A.singleton_class.equal?(A.singleton_class)`). The shell's
    /// `superclass` mirrors the real class's superclass — a Tier-1
    /// approximation of CRuby's metaclass tower that keeps
    /// `A.singleton_class.ancestors` reasonable without modelling
    /// `#<Class:A> < #<Class:Object> < …`. Single source of truth
    /// for both the `singleton_class` dispatch arm and
    /// `Op::OpenSingletonClass`.
    pub(crate) fn ensure_singleton_view(self: &Rc<Self>) -> Rc<Class> {
        use std::cell::{Cell, RefCell};
        let mut slot = self.singleton_view.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            return existing.clone();
        }
        let shell_superclass = self.superclass.borrow().clone();
        let v = Rc::new(Class {
            name: format!("#<Class:{}>", self.name),
            is_module: false,
            undefed: RefCell::new(crate::intern::FxHashSet::default()),
            anon_serial: Cell::new(0),
            ivars: RefCell::new(crate::intern::FxHashMap::default()),
            methods: RefCell::new(crate::intern::FxHashMap::default()),
            singleton_methods: RefCell::new(crate::intern::FxHashMap::default()),
            superclass: RefCell::new(shell_superclass),
            includes: RefCell::new(Vec::new()),
            prepends: RefCell::new(Vec::new()),
            singleton_prepends: RefCell::new(Vec::new()),
            singleton_includes: RefCell::new(Vec::new()),
            singleton_view: RefCell::new(None),
            singleton_target: RefCell::new(Some(Rc::downgrade(self))),
            class_vars: RefCell::new(crate::intern::FxHashMap::default()),
            consts: RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: RefCell::new(None),
            class_tag: None,
            #[cfg(feature = "cext")]
            cext_alloc_func: Cell::new(None),
        });
        *slot = Some(v.clone());
        v
    }

    /// Effective display name: the structural `name` if non-empty,
    /// otherwise the lazily-stamped `assigned_name` (set on first
    /// const-assignment per CRuby). Returns `None` for a class that
    /// is still anonymous in BOTH senses (no structural name and
    /// never const-assigned) — `Module#name` maps that to `nil`.
    pub(crate) fn effective_name(&self) -> Option<String> {
        if !self.name.is_empty() {
            Some(self.name.clone())
        } else {
            self.assigned_name.borrow().clone()
        }
    }

    /// Returns the Rc the install paths should record as
    /// `Method.defining_class` for a method being installed via
    /// this class. For an eigenclass-shell, that's the underlying
    /// real class — so `super` lookups walk the real class's
    /// singleton-method ancestor chain instead of the synthetic
    /// shell (which isn't in any superclass chain). For every
    /// other Class, it's `self`. (Code-review #253 round 1 #1.)
    pub(crate) fn effective_install_class(self: &Rc<Self>) -> Rc<Self> {
        self.singleton_target
            .borrow()
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
            .unwrap_or_else(|| self.clone())
    }
}

#[derive(Debug)]
pub struct Instance {
    pub(crate) class: Rc<Class>,
    pub(crate) ivars: IvarTable,
    /// CRuby-style eigenclass: a synthetic Class whose
    /// `superclass` is `self.class`, holding methods unique to
    /// this one object. `None` until the first singleton method
    /// is installed (`def obj.foo` or
    /// `obj.define_singleton_method(:foo)`); allocated lazily so
    /// the common case where an object never gets a singleton
    /// method pays nothing.
    ///
    /// Method lookup goes through `Heap::class_of(id)` which
    /// returns this eigenclass if present (and the eigenclass's
    /// `superclass` chain walks back to the real class
    /// transparently). `Object#class` script behaviour uses
    /// `Heap::real_class_of(id)` to skip past the eigenclass and
    /// report the original — matching CRuby, where `obj.class`
    /// returns the user-declared class, not the eigenclass.
    pub(crate) singleton_class: Option<Rc<Class>>,
    /// CRuby's per-object frozen bit. `false` by default; flipped
    /// to `true` by `Object#freeze` and stays true for the life
    /// of the object (CRuby's freeze is one-way — `unfreeze`
    /// doesn't exist). `Object#frozen?` reads this. Subsequent
    /// mutation attempts (ivar set, singleton method install,
    /// internal state mutation) should consult and raise
    /// FrozenError — currently only the freeze read/write
    /// surface is wired; full mutation guards are follow-up
    /// work. `Cell<bool>` so `freeze` on `&self` can flip
    /// without taking `&mut self`, matching the lazy
    /// singleton_class allocation convention next door.
    pub(crate) frozen: std::cell::Cell<bool>,
}

/// Per-instance variable table. The overwhelming majority of objects
/// carry only a handful of ivars, so the table is an insertion-ordered
/// small-vector scanned linearly: up to 4 ivars live INLINE in the
/// instance — no heap allocation at all, not even on the first `@x=`
/// (the old `FxHashMap` cost +59ns RawTable alloc there) — and reads are
/// linear compares, no hashing (~5x faster than the HashMap get). The
/// small-table strategy CRuby uses. Insertion order is preserved,
/// matching CRuby's `instance_variables` definition-order; a HashMap
/// spill was rejected because it would scramble that order. A
/// pathological object with hundreds of ivars spills to the SmallVec
/// heap and pays O(n) per access; the fix if it ever profiles hot is an
/// order-preserving index (this Vec + a side HashMap for lookup), not a
/// plain HashMap.
///
/// The 4-inline width is FREE on memory: `size_of::<HeapObj>()` is 136B,
/// set by `HashObj`; Instance with this table is 128B, so it stays under
/// that ceiling and the enum (sized to its max variant) does NOT grow —
/// no other slot pays for it. Speed was confirmed with an INTERLEAVED
/// A/B vs the plain-`Vec` form (alternating runs to cancel the ~40ns
/// cross-session thermal drift that made a naive before/after comparison
/// read it as a regression): C1..C4 construction −56ns, reads −14ns,
/// consistent across 12 rounds.
#[derive(Clone, Debug, Default)]
pub(crate) struct IvarTable(smallvec::SmallVec<[(crate::intern::SymId, Value); 4]>);

impl IvarTable {
    pub(crate) fn get(&self, k: &crate::intern::SymId) -> Option<&Value> {
        self.0.iter().find(|(n, _)| n == k).map(|(_, val)| val)
    }
    pub(crate) fn insert(&mut self, k: crate::intern::SymId, val: Value) -> Option<Value> {
        if let Some(slot) = self.0.iter_mut().find(|(n, _)| *n == k) {
            return Some(std::mem::replace(&mut slot.1, val));
        }
        self.0.push((k, val));
        None
    }
    pub(crate) fn remove(&mut self, k: &crate::intern::SymId) -> Option<Value> {
        self.0.iter().position(|(n, _)| n == k).map(|i| self.0.remove(i).1)
    }
    pub(crate) fn contains_key(&self, k: &crate::intern::SymId) -> bool {
        self.0.iter().any(|(n, _)| n == k)
    }
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&crate::intern::SymId, &Value)> {
        self.0.iter().map(|(k, v)| (k, v))
    }
    pub(crate) fn values(&self) -> impl Iterator<Item = &Value> {
        self.0.iter().map(|(_, v)| v)
    }
    pub(crate) fn keys(&self) -> impl Iterator<Item = &crate::intern::SymId> {
        self.0.iter().map(|(k, _)| k)
    }
}

#[derive(Debug)]
pub struct Method {
    pub(crate) params: Vec<String>,
    pub(crate) proto_idx: usize,
    pub(crate) fixed_arity: Option<FixedArity>,
    /// Class whose method table holds this Method instance — i.e.
    /// the class the `def` literally lives inside. `super` uses
    /// this class's *superclass* as the starting point for the
    /// parent-method lookup, matching CRuby's "module of
    /// definition" rule. Methods defined at the toplevel (in
    /// `<main>`, not inside any class body) have `None`; calling
    /// `super` from there raises NoMethodError.
    /// Weak ref so singleton-class methods don't form a strong
    /// cycle: an eigenclass is held only by its `Instance`'s
    /// `singleton_class` field, and each Method inside that
    /// eigenclass would otherwise pin the eigenclass back via
    /// `defining_class`. With Weak, sweeping the Instance drops
    /// the eigenclass, which drops all its Methods. Regular
    /// classes (held by `Vm.classes` for the program's lifetime)
    /// also use Weak here — the upgrade always succeeds in the
    /// regular case because `Vm.classes` keeps the strong ref.
    /// See PR #31 review for the cycle analysis.
    pub(crate) defining_class: Option<std::rc::Weak<Class>>,
    /// Method visibility. Set at `def` time from the surrounding
    /// class body's current visibility mode, but mutable post-hoc
    /// via `private :sym` / `public :sym` / `protected :sym`.
    pub(crate) visibility: Cell<Visibility>,
    /// `Some` for `define_method(:name) { ... }`-installed methods.
    /// The captured Rc is shared with the BlockHandle that the
    /// block-literal created, so closures over outer-scope locals
    /// stay live — writes by the method-call body are visible to
    /// the enclosing scope (matching CRuby's `define_method`
    /// closure semantics). `param_start` / `n_params` carry over
    /// from the BlockHandle and tell `invoke_method` where in the
    /// shared locals Vec to write the args. Methods coming from
    /// `def name ... end` have `None` here and follow the normal
    /// fresh-locals path.
    pub(crate) closure: Option<MethodClosure>,
    /// `Some` for synthesised builtin methods installed on
    /// Kernel/Object (and similar primitive-host classes) so
    /// `Kernel.instance_method(:class).arity` / `.parameters` /
    /// `.source_location` return real values instead of the
    /// `proto_idx`-derived defaults. The `proto_idx` field on
    /// these Methods points at a dummy proto (index 0) and is
    /// never read — the builtin payload supplies introspection
    /// metadata directly, and invocation short-circuits in
    /// `invoke_method_with_block` to re-enter `do_call` with the
    /// primitive name so the inline dispatch fires.
    pub(crate) builtin: Option<std::rc::Rc<BuiltinMeta>>,
    /// Captured at `def name ... end` time (or builtin synthesis).
    /// Survives `alias_method :new, :old` because alias install
    /// shares the same `Rc<Method>` — the alias's `name_id` lives
    /// on the class table; this field keeps the original def name
    /// so `Method#original_name` can return it. `None` for the few
    /// construction sites where the original name isn't available
    /// (callers fall back to the captured BoundMethod name_id).
    pub(crate) original_name: Option<crate::intern::SymId>,
}

/// Metadata for a synthesised builtin Method installed on
/// Kernel etc. so reflection sees realistic shape. Mirrors the
/// fields the arity/parameters/source_location dispatch arms
/// would otherwise derive from a real bytecode `Proto`.
#[derive(Debug)]
pub struct BuiltinMeta {
    /// The primitive method name (e.g. `:class`, `:nil?`,
    /// `:respond_to?`). Invocation pushes receiver+args onto the
    /// stack and re-enters `do_call(name_id, ...)` so the inline
    /// primitive dispatch handles it.
    pub(crate) name_id: crate::intern::SymId,
    /// CRuby `Method#arity` value. Negative means "variadic":
    /// `-(required + 1)` per CRuby's encoding.
    pub(crate) arity: i64,
    /// CRuby `Method#parameters` shape: list of (kind, name)
    /// pairs where kind is `"req"`, `"opt"`, `"rest"`, etc.
    pub(crate) parameters: Vec<(&'static str, Option<String>)>,
    /// `Method#source_location` label. `Some(label)` emits the
    /// `[label, line]` array. `None` emits `nil` — CRuby's shape
    /// for some C-defined methods (e.g.
    /// `BasicObject.instance_method(:__id__).source_location`
    /// returns nil even though Kernel's `:class` returns
    /// `["<internal:kernel>", 18]`).
    pub(crate) source_label: Option<&'static str>,
    /// Line on the source label when `source_label` is `Some`.
    /// CRuby uses real C-file line numbers; we use `0` as a
    /// stable placeholder. Ignored when `source_label` is `None`.
    pub(crate) source_line: i64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedArity {
    pub(crate) required: u16,
    pub(crate) n_locals: u16,
    /// Cached `!proto.creates_block` — `Locals::Stack` eligibility for
    /// the dispatch fast paths. Lives here (not read from the Proto at
    /// call time) so the hot paths don't take a ~320-byte-stride
    /// `protos[idx]` cache miss per call; the Method (and its
    /// fixed_arity) is already in hand. Builtin-synthesised methods
    /// set `false` defensively (their `proto_idx` is a placeholder).
    pub(crate) stack_eligible: bool,
}

#[derive(Debug, Clone)]
pub struct MethodClosure {
    pub(crate) captured: Rc<RefCell<Vec<Value>>>,
    pub(crate) param_start: u16,
    pub(crate) n_params: u16,
    /// The yield-block the source block lexically captured (the
    /// `block_arg` of the method that ran `define_method`), copied
    /// from the `BlockHandle` at install time. CRuby treats a
    /// `define_method` body as a Proc: `yield` inside it does NOT
    /// reach the CALLER's block (so the invocation frame keeps
    /// `block_arg: None`), but DOES reach the block active where the
    /// define_method block was created. Restored onto the invocation
    /// frame's `captured_yield_block` so that lexical `yield`
    /// resolves; `None` (→ LocalJumpError on `yield`) when the
    /// enclosing scope had no block. GC-rooted wherever `captured`
    /// is.
    pub(crate) captured_yield_block: Option<ObjId>,
}

/// Serde bridge for the LITERAL subset of `Value` — only the shapes
/// the compiler stores in `Proto::kw_param_defaults` (see
/// `expr_is_compile_time_literal` in compiler.rs): Nil / Bool / Int
/// / Float / Sym / Str. Heap-referencing variants (Object / Array /
/// Hash / ObjId-carrying anything) are NOT serializable — an ObjId
/// is only meaningful inside one live heap — and encoding one is a
/// hard error so the preamble cache falls back to the live compile
/// path rather than persisting a dangling reference.
///
/// Strings round-trip through bytes: valid UTF-8 reconstructs via
/// `Value::new_str` and invalid via `Value::new_str_bytes`, the
/// same split the compiler makes between `Expr::StrLit` and
/// `Expr::StrLitBytes` when it built the literal in the first
/// place, so the rebuilt RStr matches the original's constructor
/// path (including the encoding tag).
#[cfg(feature = "preamble-cache")]
mod preamble_cache_serde {
    use super::Value;

    #[derive(serde::Serialize, serde::Deserialize)]
    enum LitValue {
        Nil,
        Bool(bool),
        Int(i64),
        Float(f64),
        Sym(crate::intern::SymId),
        StrBytes(Vec<u8>),
    }

    impl serde::Serialize for Value {
        fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
            use serde::ser::Error;
            let lit = match self {
                Value::Nil => LitValue::Nil,
                Value::Bool(b) => LitValue::Bool(*b),
                Value::Int(n) => LitValue::Int(*n),
                Value::Float(f) => LitValue::Float(*f),
                Value::Sym(id) => LitValue::Sym(*id),
                Value::Str(s) => LitValue::StrBytes(s.borrow().clone()),
                other => {
                    return Err(S::Error::custom(format!(
                        "non-literal Value ({}) cannot be serialized",
                        other.type_name(),
                    )));
                }
            };
            lit.serialize(ser)
        }
    }

    impl<'de> serde::Deserialize<'de> for Value {
        fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
            Ok(match LitValue::deserialize(de)? {
                LitValue::Nil => Value::Nil,
                LitValue::Bool(b) => Value::Bool(b),
                LitValue::Int(n) => Value::Int(n),
                LitValue::Float(f) => Value::Float(f),
                LitValue::Sym(id) => Value::Sym(id),
                LitValue::StrBytes(b) => match String::from_utf8(b) {
                    Ok(s) => Value::new_str(s),
                    Err(e) => Value::new_str_bytes(e.into_bytes()),
                },
            })
        }
    }
}

#[cfg(all(test, feature = "preamble-cache"))]
mod preamble_cache_serde_tests {
    use super::Value;

    fn roundtrip(v: &Value) -> Value {
        let bytes = postcard::to_allocvec(v).expect("encode");
        postcard::from_bytes(&bytes).expect("decode")
    }

    #[test]
    fn literal_variants_roundtrip() {
        assert!(matches!(roundtrip(&Value::Nil), Value::Nil));
        assert!(matches!(roundtrip(&Value::Bool(true)), Value::Bool(true)));
        assert!(matches!(roundtrip(&Value::Bool(false)), Value::Bool(false)));
        assert!(matches!(roundtrip(&Value::Int(-42)), Value::Int(-42)));
        assert!(matches!(roundtrip(&Value::Float(1.5)), Value::Float(f) if (f - 1.5).abs() < 1e-12));
        let sym = Value::Sym(crate::intern::SymId(7));
        assert!(matches!(roundtrip(&sym), Value::Sym(crate::intern::SymId(7))));
    }

    #[test]
    fn utf8_string_rebuilds_via_new_str() {
        let v = Value::new_str("héllo");
        assert!(matches!(roundtrip(&v), Value::Str(s) if s.borrow().as_slice() == "héllo".as_bytes()));
    }

    #[test]
    fn binary_string_rebuilds_via_new_str_bytes() {
        // Invalid UTF-8 — the compiler's `Expr::StrLitBytes` shape.
        let v = Value::new_str_bytes(vec![0xFF, 0xFE, 0x00]);
        assert!(matches!(roundtrip(&v), Value::Str(s) if s.borrow().as_slice() == b"\xFF\xFE\x00"));
    }

    #[test]
    fn heap_variants_refuse_to_serialize() {
        // An ObjId is only meaningful inside one live heap — the
        // bridge must hard-error, which the preamble cache treats
        // as "skip storing", never persisting a dangling reference.
        let v = Value::Array(crate::value::ObjId(3));
        assert!(postcard::to_allocvec(&v).is_err());
    }
}
