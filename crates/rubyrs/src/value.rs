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
    /// Cached UTF-8 VALIDITY of the current content: -1 unknown, 0
    /// invalid, 1 valid. `std::str::from_utf8` is O(n), and the
    /// char-indexed String ops (`[]`, `match(re, pos)`) ran it (or a
    /// lossy equivalent) on EVERY call — on rubocop's 21KB source
    /// buffer that validation alone was ~600ns/call × 8.6k
    /// `Buffer#slice` calls per walk. Same invalidation contract as
    /// `ascii_cache` (cleared by `borrow_mut`); ASCII-only content is
    /// trivially valid, so `is_utf8_cached` consults the ASCII flag
    /// first and this cell is only computed for non-ASCII content.
    utf8_cache: Cell<i8>,
    /// Lazily-built char→byte offset table for content that receives
    /// CHAR-indexed ops (`String#[]` / `match(re, pos)` / `match?`):
    /// entry `i` is the byte offset where char `i` starts, plus one
    /// final entry == `bytes.len()`, so the byte span of chars
    /// `[a, b)` is `starts[a]..starts[b]` — an O(1) lookup instead of
    /// the O(n) `chars().collect()` walk the generic paths did per
    /// call. Only strings of `CHAR_STARTS_CACHE_MIN`+ bytes cache the
    /// table (below that the direct walk is cheaper than the alloc
    /// churn); ASCII-only strings never need it (byte == char).
    /// Cleared by `borrow_mut` (same contract as `hash_cache`).
    char_starts: RefCell<Option<Rc<Vec<u32>>>>,
}

/// Content-size threshold for CACHING the char→byte table. Small
/// strings rebuild it per call (still cheap); large buffers — the
/// rubocop source-buffer case — keep it until the next mutation.
const CHAR_STARTS_CACHE_MIN: usize = 256;

impl StrCell {
    #[inline]
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: RefCell::new(bytes),
            hash_cache: Cell::new(0),
            ascii_cache: Cell::new(-1),
            utf8_cache: Cell::new(-1),
            char_starts: RefCell::new(None),
        }
    }

    /// Read access — passthrough to the inner `RefCell`.
    #[inline]
    pub fn borrow(&self) -> std::cell::Ref<'_, Vec<u8>> {
        self.bytes.borrow()
    }

    /// Write access — clears the cached hash, ASCII/UTF-8 flags AND
    /// the char→byte table BEFORE handing out the guard (see the
    /// invalidation contract above).
    #[inline]
    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, Vec<u8>> {
        self.hash_cache.set(0);
        self.ascii_cache.set(-1);
        self.utf8_cache.set(-1);
        if self.char_starts.borrow().is_some() {
            *self.char_starts.borrow_mut() = None;
        }
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

    /// Is the content valid UTF-8? Caches the O(n) validation; the
    /// cache is reset by `borrow_mut`. ASCII-only content (per the
    /// ASCII cache) short-circuits to `true` without touching the
    /// UTF-8 cell.
    #[inline]
    pub(crate) fn is_utf8_cached(&self) -> bool {
        if self.ascii_cache.get() == 1 {
            return true;
        }
        match self.utf8_cache.get() {
            1 => true,
            0 => false,
            _ => {
                let ok = std::str::from_utf8(&self.bytes.borrow()).is_ok();
                self.utf8_cache.set(if ok { 1 } else { 0 });
                ok
            }
        }
    }

    /// The char→byte offset table for the CURRENT content (see the
    /// field doc): `starts[i]` = byte offset of char `i`, one extra
    /// final entry == byte length. Chars follow the same walk the
    /// invalid-UTF-8 `String#[]` arm has always used (well-formed
    /// sequences advance by their length; a malformed lead /
    /// continuation byte counts as its own 1-byte char) — for VALID
    /// UTF-8 that is exactly the `char_indices` boundaries. Cached
    /// for large buffers, rebuilt per call for small ones.
    pub(crate) fn char_starts(&self) -> Rc<Vec<u32>> {
        if let Some(cs) = self.char_starts.borrow().as_ref() {
            return cs.clone();
        }
        let bytes = self.bytes.borrow();
        let mut starts: Vec<u32> = Vec::with_capacity(bytes.len().min(1 << 20) + 1);
        let mut i = 0;
        while i < bytes.len() {
            starts.push(i as u32);
            let b = bytes[i];
            let seq = if b < 0x80 {
                1
            } else if b & 0xE0 == 0xC0 {
                2
            } else if b & 0xF0 == 0xE0 {
                3
            } else if b & 0xF8 == 0xF0 {
                4
            } else {
                1 // continuation byte or invalid lead → its own 1-byte char
            };
            let well_formed = seq > 1
                && i + seq <= bytes.len()
                && (1..seq).all(|k| bytes[i + k] & 0xC0 == 0x80);
            i += if well_formed { seq } else { 1 };
        }
        starts.push(bytes.len() as u32);
        let cache_it = bytes.len() >= CHAR_STARTS_CACHE_MIN;
        drop(bytes);
        let table = Rc::new(starts);
        if cache_it {
            *self.char_starts.borrow_mut() = Some(table.clone());
        }
        table
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

// `#[repr(u8)]` pins the in-memory layout (ADR 0035 Phase 1): a `u8` discriminant at offset
// 0, then each variant's fields at their natural alignment — so an ObjId variant is `{u8
// tag, u32 oid}` with `oid` at offset 4, while `{u8 tag, i64}` puts its payload at offset 8.
// `size_of::<Value>() == 16` (the widest variant is `{u8, i64/f64/Rc}` → 8-aligned, 16 bytes).
// Without a `#[repr]` the offset is compiler-chosen and unstable, so the native JIT cannot
// read an Object's `oid` with an inline load — it must call a primitive. The asserts below
// guard the contract the JIT relies on; see `value_layout_contract` for the offset test
// (`OID_OFFSET == 4`).
#[derive(Clone, Debug)]
#[repr(u8)]
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

// ADR 0035 Phase 1 — the layout contract the native JIT will rely on. A change that grows
// `Value` past 16 bytes, or moves the payload off offset 8, breaks inline object access and
// must be a deliberate decision (re-derive the JIT's offsets), not a silent regression.
const _: () = assert!(std::mem::size_of::<Value>() == 16, "Value must stay 16 bytes (ADR 0035)");
const _: () = assert!(std::mem::align_of::<Value>() == 8, "Value must stay 8-aligned (ADR 0035)");

/// The canonical-owner chain for a closure's captured (outer) slot
/// region. Element `i` is `(cell, start)`: `cell` is the locals cell
/// that OWNS slots `[start_i, start_{i+1})` (the last element owns up
/// to the closure's own `param_start`), and `chain[0].1 == 0` always
/// — the root method / class-body / toplevel scope. Built once at
/// `Op::CreateBlock` (a block created in a non-block scope gets the
/// one-element chain `[(scope_cell, 0)]`; a block created inside a
/// block frame extends the creating frame's chain with that frame's
/// own per-invocation cell). Because each element holds a strong Rc,
/// the ORIGINAL binding cells stay alive — and reads/writes of a
/// captured local can be routed to the canonical cell — for the
/// lifetime of any capturing closure, including after the defining
/// frames pop (the shared-binding semantics CRuby closures have).
pub(crate) type OuterChain = Rc<[(Rc<RefCell<Vec<Value>>>, u16)]>;

/// The cell that canonically owns `slot` per `chain` — the LAST
/// element whose start is `<= slot`. Chains are tiny (nesting depth
/// of the source), so a reverse linear scan beats anything fancier.
#[inline]
pub(crate) fn chain_owner_cell(
    chain: &[(Rc<RefCell<Vec<Value>>>, u16)],
    slot: usize,
) -> &Rc<RefCell<Vec<Value>>> {
    for (cell, start) in chain.iter().rev() {
        if (*start as usize) <= slot {
            return cell;
        }
    }
    // chain[0].1 == 0 by construction, so the loop always returns;
    // defensive fallback to the root keeps this off the panic budget.
    &chain[0].0
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
    /// ANCESTOR canonical-owner chain for the block's outer slot
    /// region — the scopes OUTSIDE the creating scope. Together with
    /// `(captured, creator_start)` this describes every captured
    /// binding: slot `>= creator_start` (and `< param_start`) lives
    /// in `captured` (the creating scope's cell); slot `<
    /// creator_start` routes through this chain (see [`OuterChain`]).
    /// `None` when the creating scope canonically owns everything —
    /// a method / class-body / toplevel creator (the overwhelmingly
    /// common case) and synthetic forwarder handles — which keeps
    /// `Op::CreateBlock` allocation-free for depth-1 blocks. GC:
    /// rooted alongside `captured` (see `each_capture_cell`).
    pub(crate) outer_chain: Option<OuterChain>,
    /// First slot index the CREATING scope's cell (`captured`)
    /// canonically owns — `0` for a root-scope creator; the creating
    /// block frame's `own_start` when this block was created inside
    /// another block / define_method body. Invariant: `Some` chain ⇔
    /// `creator_start > 0`.
    pub(crate) creator_start: u16,
}

impl BlockHandle {
    /// Every locals cell this handle can reach: `captured` plus each
    /// distinct ancestor-chain cell. GC root walks use this so heap
    /// values reachable only through an ORIGINAL binding cell (whose
    /// defining frame already popped) survive collection.
    pub(crate) fn each_capture_cell(&self, mut f: impl FnMut(&Rc<RefCell<Vec<Value>>>)) {
        f(&self.captured);
        if let Some(chain) = &self.outer_chain {
            for (cell, _) in chain.iter() {
                if !Rc::ptr_eq(cell, &self.captured) {
                    f(cell);
                }
            }
        }
    }
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
    /// ADR 0035 Phases 4/5 — the per-class UNION ivar-shape table:
    /// `@name → slot index` shared by every instance of this class.
    /// Built on first assignment of each name (any instance);
    /// MONOTONIC — names only ever ADD, and a slot index, once handed
    /// out, is permanent for the class's lifetime. That monotonicity
    /// is what lets inline caches and the JIT bake `(class_ptr, slot)`
    /// pairs with NO invalidation protocol: a cached slot can never go
    /// stale, only a cached class_ptr can mismatch. Instances store
    /// their ivars in a slot-indexed array (`IvarTable`) so a shape-
    /// guarded access is a direct offset load instead of a scan.
    /// Instances that assign in different orders share the same table
    /// (union semantics); each instance's own assignment ORDER is kept
    /// on the instance (`IvarTable::order`) for `instance_variables`.
    pub(crate) ivar_shape: RefCell<IvarShape>,
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
            // Clone the shape so any instance table that ends up keyed
            // to the copy keeps a consistent slot numbering (a shape
            // superset is always safe; sharing a RefCell would not be).
            ivar_shape: RefCell::new(self.ivar_shape.borrow().clone()),
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
            ivar_shape: RefCell::new(IvarShape::default()),
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

    /// ADR 0035 Ph4/5 — resolve an ivar name to its slot in this
    /// class's union shape, WITHOUT adding it (read/reflection paths:
    /// an unknown name means "no instance of this class ever assigned
    /// it" → undefined ivar).
    #[inline]
    pub(crate) fn ivar_slot_lookup(&self, sym: SymId) -> Option<u32> {
        self.ivar_shape.borrow().lookup(sym)
    }

    /// Resolve-or-add an ivar name in this class's union shape (write
    /// paths). The returned slot is permanent for this class.
    #[inline]
    pub(crate) fn ivar_slot_intern(&self, sym: SymId) -> u32 {
        self.ivar_shape.borrow_mut().intern(sym)
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

impl Instance {
    /// Ivar read by name via the class shape (undefined → None; an
    /// assigned nil is `Some(&Nil)` — reflection needs the difference).
    #[inline]
    pub(crate) fn ivar_get(&self, sym: SymId) -> Option<&Value> {
        self.ivars.get(&self.class, sym)
    }
    /// Ivar write by name; interns the name into the class shape on
    /// first assignment anywhere in the class.
    #[inline]
    pub(crate) fn ivar_set(&mut self, sym: SymId, val: Value) -> Option<Value> {
        let slot = self.class.ivar_slot_intern(sym);
        self.ivars.write_slot(slot, val)
    }
    #[inline]
    pub(crate) fn ivar_remove(&mut self, sym: SymId) -> Option<Value> {
        let class = self.class.clone();
        self.ivars.remove(&class, sym)
    }
    #[inline]
    pub(crate) fn ivar_defined(&self, sym: SymId) -> bool {
        self.ivars.contains_key(&self.class, sym)
    }
    /// Defined ivar names in THIS object's assignment order (CRuby's
    /// `instance_variables` contract).
    pub(crate) fn ivar_names(&self) -> Vec<SymId> {
        self.ivars.keys(&self.class)
    }
    /// `(name, &value)` pairs in assignment order.
    pub(crate) fn ivar_pairs(&self) -> Vec<(SymId, &Value)> {
        self.ivars.iter(&self.class)
    }
}

/// ADR 0035 Phases 4/5 — a class's union ivar shape: `names[slot]` is
/// the ivar name owning `slot`; `map` is the reverse probe, consulted
/// once `names` outgrows the linear-scan threshold (a handful of `u32`
/// compares beats a hash probe for the typical ≤8-ivar class). Both are
/// maintained together from the first insert. See `Class::ivar_shape`
/// for the monotonicity contract that makes baked slots safe.
#[derive(Clone, Debug, Default)]
pub(crate) struct IvarShape {
    names: Vec<SymId>,
    map: FxHashMap<SymId, u32>,
}

/// Above this many names, `IvarShape::lookup` switches from the linear
/// name scan to the hash probe.
const SHAPE_LINEAR_MAX: usize = 8;

impl IvarShape {
    #[inline]
    pub(crate) fn lookup(&self, sym: SymId) -> Option<u32> {
        if self.names.len() <= SHAPE_LINEAR_MAX {
            self.names.iter().position(|s| *s == sym).map(|i| i as u32)
        } else {
            self.map.get(&sym).copied()
        }
    }
    pub(crate) fn intern(&mut self, sym: SymId) -> u32 {
        if let Some(s) = self.lookup(sym) {
            return s;
        }
        let slot = u32::try_from(self.names.len()).expect("ICE: > u32::MAX ivar names on one class");
        self.names.push(sym);
        self.map.insert(sym, slot);
        slot
    }
    #[inline]
    pub(crate) fn name_of(&self, slot: u32) -> SymId {
        self.names[slot as usize]
    }
}

/// Per-instance variable storage (ADR 0035 Phases 4/5 — FLAT layout).
///
/// `slots` is indexed by the owning class's union shape
/// (`Class::ivar_shape`): every instance of class C stores `@name` at
/// the same index, so a shape-guarded access is ONE offset load — no
/// scan, no hash. The vector grows lazily to the highest slot this
/// object has assigned; slots below that which the object never
/// assigned are HOLES holding `Value::Nil` (so the hot read path can
/// load `slots[slot]` raw — an undefined ivar correctly reads as nil
/// without consulting the defined-set).
///
/// `order` lists this object's DEFINED slots in assignment order —
/// CRuby's `instance_variables` order (which is per-object, not
/// per-class: two instances assigning in different orders report
/// differently) — and doubles as the defined-set for iteration and
/// `len`. `bits` is the O(1) defined test for slots < 64 (write paths
/// need "first assignment?" per store; slots ≥ 64 fall back to an
/// `order` scan — pathological classes only).
///
/// Invariants:
///   - `defined(s)` ⟺ `s ∈ order`  (bits mirrors this for s < 64)
///   - `!defined(s) && s < slots.len()` ⟹ `slots[s] == Nil`
///     (`remove` writes Nil back so a hole never leaks a stale value
///     to the raw read path, and the GC never marks removed values)
///
/// Memory: 4 inline value slots + 4 inline order entries + the bitset
/// = 104 bytes, the same as the previous scan-table (`SmallVec<[(sym,
/// val); 4]>` at 24B stride) — `Instance` stays under the `HashObj`
/// ceiling and `HeapObj` does not grow.
#[derive(Clone, Debug, Default)]
pub(crate) struct IvarTable {
    slots: smallvec::SmallVec<[Value; 4]>,
    order: smallvec::SmallVec<[u32; 4]>,
    bits: u64,
}

impl IvarTable {
    /// Build a table for an instance of `class` from `(name, value)`
    /// pairs in assignment order — the native object builders
    /// (prism materialize / exception construction) accumulate pairs
    /// before the class is resolved, then materialize here.
    pub(crate) fn from_pairs(
        class: &Class,
        pairs: impl IntoIterator<Item = (SymId, Value)>,
    ) -> IvarTable {
        let mut t = IvarTable::default();
        for (s, v) in pairs {
            t.insert(class, s, v);
        }
        t
    }

    #[inline]
    fn defined(&self, slot: u32) -> bool {
        if slot < 64 {
            self.bits & (1u64 << slot) != 0
        } else {
            self.order.contains(&slot)
        }
    }

    pub(crate) fn get(&self, class: &Class, k: SymId) -> Option<&Value> {
        let slot = class.ivar_slot_lookup(k)?;
        self.read_slot(slot)
    }
    /// Defined-aware slot read (reflection: distinguishes an assigned
    /// nil from an undefined ivar).
    #[inline]
    pub(crate) fn read_slot(&self, slot: u32) -> Option<&Value> {
        if (slot as usize) < self.slots.len() && self.defined(slot) {
            Some(&self.slots[slot as usize])
        } else {
            None
        }
    }
    /// Hot-path read for `@x` (interpreter IC hit / JIT): holes and
    /// never-grown slots read as Nil — exactly CRuby's undefined-ivar
    /// semantics — with no defined-set consulted.
    #[allow(dead_code)] // wired by the ivar-IC step arms
    #[inline]
    pub(crate) fn read_slot_raw(&self, slot: u32) -> Value {
        self.slots.get(slot as usize).cloned().unwrap_or(Value::Nil)
    }
    pub(crate) fn insert(&mut self, class: &Class, k: SymId, val: Value) -> Option<Value> {
        let slot = class.ivar_slot_intern(k);
        self.write_slot(slot, val)
    }
    /// Slot-level store (IC hit / JIT helpers / `insert`). Grows the
    /// slot vector (Nil-filling holes) on first touch past the end.
    pub(crate) fn write_slot(&mut self, slot: u32, val: Value) -> Option<Value> {
        let i = slot as usize;
        if i >= self.slots.len() {
            self.slots.resize(i + 1, Value::Nil);
        }
        if self.defined(slot) {
            Some(std::mem::replace(&mut self.slots[i], val))
        } else {
            if slot < 64 {
                self.bits |= 1u64 << slot;
            }
            self.order.push(slot);
            self.slots[i] = val;
            None
        }
    }
    pub(crate) fn remove(&mut self, class: &Class, k: SymId) -> Option<Value> {
        let slot = class.ivar_slot_lookup(k)?;
        if (slot as usize) >= self.slots.len() || !self.defined(slot) {
            return None;
        }
        if slot < 64 {
            self.bits &= !(1u64 << slot);
        }
        if let Some(p) = self.order.iter().position(|s| *s == slot) {
            self.order.remove(p);
        }
        // Nil-out so the hole invariant holds (raw reads + GC marking).
        Some(std::mem::replace(&mut self.slots[slot as usize], Value::Nil))
    }
    pub(crate) fn contains_key(&self, class: &Class, k: SymId) -> bool {
        class
            .ivar_slot_lookup(k)
            .is_some_and(|s| (s as usize) < self.slots.len() && self.defined(s))
    }
    #[allow(dead_code)] // kept for API completeness with is_empty()
    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }
    #[allow(dead_code)] // kept for API completeness
    pub(crate) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    /// Defined values in assignment order — classless, so the GC mark
    /// path stays a plain iteration (holes are Nil and never visited).
    pub(crate) fn values(&self) -> impl Iterator<Item = &Value> {
        self.order.iter().map(|&s| &self.slots[s as usize])
    }
    /// Defined names in assignment order (needs the owning class to
    /// resolve slot → name).
    pub(crate) fn keys(&self, class: &Class) -> Vec<SymId> {
        let shape = class.ivar_shape.borrow();
        self.order.iter().map(|&s| shape.name_of(s)).collect()
    }
    /// `(name, value)` pairs in assignment order. Collected (not lazy)
    /// because the name resolution holds the class shape borrow.
    pub(crate) fn iter<'a>(&'a self, class: &Class) -> Vec<(SymId, &'a Value)> {
        let shape = class.ivar_shape.borrow();
        self.order
            .iter()
            .map(|&s| (shape.name_of(s), &self.slots[s as usize]))
            .collect()
    }
    /// ADR 0035 Phase 4/5 — the contiguous SLOT array for the JIT's
    /// inline offset load: base pointer (`SmallVec::as_ptr`, valid
    /// whether inline or spilled) + live length, stride
    /// `size_of::<Value>()` (16). The compiled code guards
    /// `slot < len` and loads `base + slot*16`; holes are Nil so an
    /// undefined ivar reads nil / deopts on a kind check, matching the
    /// interpreter. Valid for the GC-free duration of a compiled
    /// method (the heap does not move while one runs).
    #[cfg(feature = "jit-native")]
    pub(crate) fn as_ptr_len(&self) -> (*const Value, usize) {
        (self.slots.as_ptr(), self.slots.len())
    }
}

// The JIT bakes the slot stride — pin Value's size (16B, also pinned
// by the Phase-1 layout contract) and keep the table at its budget.
const _: () = assert!(std::mem::size_of::<Value>() == 16);
const _: () = assert!(std::mem::size_of::<IvarTable>() <= 104);

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
    /// ANCESTOR canonical-owner chain + creating-scope boundary —
    /// same split and semantics as `BlockHandle::outer_chain` /
    /// `creator_start` (copied from the source handle at install
    /// time). Snapshot-restored / synthetic closures carry `(None,
    /// 0)`: the captured cell owns everything (pre-chain behaviour).
    pub(crate) outer_chain: Option<OuterChain>,
    pub(crate) creator_start: u16,
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

impl MethodClosure {
    /// Every locals cell this closure can reach — `captured` plus
    /// each distinct outer-chain cell. Mirror of
    /// `BlockHandle::each_capture_cell` for the GC root walks.
    pub(crate) fn each_capture_cell(&self, mut f: impl FnMut(&Rc<RefCell<Vec<Value>>>)) {
        f(&self.captured);
        if let Some(chain) = &self.outer_chain {
            for (cell, _) in chain.iter() {
                if !Rc::ptr_eq(cell, &self.captured) {
                    f(cell);
                }
            }
        }
    }
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

#[cfg(test)]
mod value_layout_contract {
    //! ADR 0035 Phase 1 — pins the `#[repr(u8)]` layout the native JIT relies on to read an
    //! Object's `oid` with an inline load instead of a primitive call. If `#[repr(u8)]` ever
    //! changes or a fatter payload moves the offset, these fail LOUDLY (with the real offset)
    //! so the JIT's baked offsets can be re-derived deliberately.
    use super::{ObjId, Value};

    /// `#[repr(u8)]` lays each variant out independently as `{ u8 tag @0, fields… }`, so the
    /// `u32` `oid` of an ObjId-carrying variant sits at offset 4 (the next 4-aligned slot
    /// after the tag) — while `i64`/`f64` payloads land at offset 8. The JIT extracts `oid`
    /// as a `u32` at `OID_OFFSET`, trusting the tracked `Kind` for the variant, so every
    /// ObjId-carrying variant must agree on it (they do: all are `{u8, u32}`).
    pub(crate) const OID_OFFSET: usize = 4;
    /// The discriminant byte (the `#[repr(u8)]` tag) is at offset 0. Its VALUES shift with
    /// cfg-gated variants (bignum/regex/rational), so a phase that wants an inline tag CHECK
    /// must read the live value at runtime, not bake a constant — `OID_OFFSET` is what is
    /// stable. Exposed so `Value::OBJECT_TAG`-style probes can derive it if ever needed.
    pub(crate) const TAG_OFFSET: usize = 0;

    #[test]
    fn object_oid_is_at_offset_4() {
        assert_eq!(std::mem::size_of::<Value>(), 16);
        assert_eq!(std::mem::align_of::<Value>(), 8);
        const MAGIC: u32 = 0xCAFE_F00D;
        // Every ObjId-carrying variant exposes its u32 at the same offset (the JIT extracts
        // `oid` by offset, trusting the tracked `Kind`). A `{u8 tag, i64}` variant keeps its
        // payload at offset 8 — checked here so the two offsets can't silently converge.
        for v in [
            Value::Object(ObjId(MAGIC)),
            Value::Array(ObjId(MAGIC)),
            Value::Hash(ObjId(MAGIC)),
        ] {
            let got = unsafe { *((&v as *const Value as *const u8).add(OID_OFFSET) as *const u32) };
            assert_eq!(got, MAGIC, "ObjId payload not at offset 4 — JIT contract broken");
        }
        let iv = Value::Int(0x0123_4567_89AB_CDEF);
        let got = unsafe { *((&iv as *const Value as *const u8).add(8) as *const i64) };
        assert_eq!(got, 0x0123_4567_89AB_CDEF, "Int payload not at offset 8");
        assert_eq!(TAG_OFFSET, 0);
    }
}
