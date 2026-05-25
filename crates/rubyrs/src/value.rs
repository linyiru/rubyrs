use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::intern::SymId;

/// Heap-shared string body with a frozen flag. Wraps a
/// `RefCell<String>` so that aliases see mutations, and a
/// `Cell<bool>` so `freeze` / `frozen?` round-trip without
/// touching the content's borrow. Derefs to the inner RefCell —
/// existing `.borrow()` / `.borrow_mut()` calls keep their
/// terse form; the frozen flag rides as a sibling on the Rc.
#[derive(Debug)]
pub struct RStr {
    pub(crate) content: RefCell<String>,
    pub(crate) frozen: Cell<bool>,
}

impl RStr {
    pub fn new(s: String) -> Self {
        Self { content: RefCell::new(s), frozen: Cell::new(false) }
    }
}

impl std::ops::Deref for RStr {
    type Target = RefCell<String>;
    fn deref(&self) -> &Self::Target { &self.content }
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
    /// 64-bit float. Mixed arithmetic with Int promotes the Int
    /// (CRuby's "Float wins on mix" rule). Equality across the
    /// numeric types coerces too — `5 == 5.0` is `true`.
    Float(f64),
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
    /// `/pattern/` literal. The compiled `regex::Regex` is shared
    /// via Rc — Regex is immutable so there's no aliasing risk.
    /// Rust's regex crate uses a different dialect from Onigmo
    /// (CRuby's engine); the gaps (possessive quantifiers,
    /// `\k<name>` backrefs, look-around in some forms) are
    /// documented in SUBSET.md.
    Regex(std::rc::Rc<regex::Regex>),
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
    pub(crate) param_start: u16,
    pub(crate) n_params: u16,
    /// `Some(slot)` when the block declares a `*rest` parameter.
    /// `slot` is the local-slot index where the rest collector
    /// lives. Filled by `invoke_block` with a fresh Array of any
    /// args past the last required slot. `None` means no rest —
    /// overflow args are silently dropped (CRuby behaviour for
    /// blocks).
    pub(crate) rest_slot: Option<u16>,
}

#[derive(Debug)]
pub struct Class {
    pub(crate) name: String,
    pub(crate) methods: RefCell<HashMap<SymId, Rc<Method>>>,
    /// Per-class singleton-method table — `def self.foo; ...; end`
    /// inside a class body installs `foo` here. Dispatched against
    /// `Value::Class(c)` receivers in `do_call`. Parallel to
    /// `cext_class_methods` (which holds C-ext-installed singletons
    /// keyed by class joined name); this one holds user-Ruby
    /// singletons keyed by interned method SymId on the Class
    /// itself, so it survives class re-opening naturally and
    /// doesn't need a separate generation counter.
    pub(crate) singleton_methods: RefCell<HashMap<SymId, Rc<Method>>>,
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
}

#[derive(Debug)]
pub struct Instance {
    pub(crate) class: Rc<Class>,
    pub(crate) ivars: HashMap<SymId, Value>,
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
}

#[derive(Debug)]
pub struct Method {
    pub(crate) params: Vec<String>,
    pub(crate) proto_idx: usize,
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
}

#[derive(Debug, Clone)]
pub struct MethodClosure {
    pub(crate) captured: Rc<RefCell<Vec<Value>>>,
    pub(crate) param_start: u16,
    pub(crate) n_params: u16,
}
