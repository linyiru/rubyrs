//! Centralized encoding of the "this constant path is absolute"
//! signal that flows from the AST lowering through the compiler
//! and into the runtime.
//!
//! Background: CRuby distinguishes `Foo::Bar` (relative — cref-walk
//! the enclosing scopes) from `::Foo::Bar` (absolute — look up the
//! joined name at top level only). Internally rubyrs's AST/compiler
//! signals this by prefixing the joined name with `::` before
//! interning it as a `String` / `SymId`. PR #355 introduced the
//! convention for `Expr::ConstRead` reads and class superclass
//! emit; PR #355 cycle 4 extended it to the three op-write
//! variants (`+=`, `||=`, `&&=`); PR #370 extended it to the
//! rescue-clause class list and the `PushRescue` runtime handler.
//!
//! With the convention spread across ten sites (six AST producers
//! plus four runtime/compile consumers), inline `format!("::{}", ..)`
//! and `name.strip_prefix("::")` were drifting apart — code-review
//! cycles flagged the asymmetry as an altitude risk: any future
//! work that touches the marker semantics (autoload trigger,
//! reflection on `Symbol#to_s`, debug dumps, alternate marker
//! character) has to find and update every site by hand.
//!
//! This module makes the convention explicit: a single constant
//! defines the marker, and two helpers (`tag_absolute` /
//! `strip_absolute`) own the encode/decode contract for that
//! AST→compiler→VM channel.
//!
//! Scope clarification: the helpers cover the *internal marker*
//! convention only — the leading `::` that the AST lowering
//! attaches to a joined constant-path name before interning it.
//! Producers in `ast.rs` and consumers in `compiler.rs` / `vm/step.rs`
//! (`PushRescue`) all route through this module.
//!
//! Out of scope, even though they also inspect a literal `::`
//! prefix: places that parse a Ruby surface-syntax string at
//! runtime, e.g. `Vm::resolve_const_path` powering
//! `Module#const_get("::Foo::Bar")` / `Module#const_defined?`.
//! That path's `::` comes from the user-visible Ruby string, not
//! from this module's internal tagging convention — sharing a
//! helper would conflate two different concerns. They are kept
//! deliberately separate; if a future change broadens the marker
//! semantics, audit both surfaces independently.
//!
//! The helpers operate on `String` / `&str` only — they don't
//! change the carrier types or remove the marker from interned
//! `SymId`s. A deeper structural refactor (e.g., changing
//! `Expr::ConstRead(String)` to a struct variant with an explicit
//! `absolute: bool` field, and threading the bit through the
//! bytecode) would remove the marker from the interner entirely
//! but touches every Expr matcher in the codebase. Deferred until
//! profiling or further review demands it.

/// Marker prefix the AST attaches to absolute constant-path names
/// (`::Foo::Bar`). Chosen to mirror the Ruby surface syntax so the
/// interned form is greppable in debug output.
pub(crate) const ABSOLUTE_PREFIX: &str = "::";

/// Tag `name` as absolute when `absolute` is true; return it
/// unchanged otherwise so the relative path stays allocation-free.
pub(crate) fn tag_absolute(name: String, absolute: bool) -> String {
    if absolute {
        let mut tagged = String::with_capacity(ABSOLUTE_PREFIX.len() + name.len());
        tagged.push_str(ABSOLUTE_PREFIX);
        tagged.push_str(&name);
        tagged
    } else {
        name
    }
}

/// Strip the absolute marker if present. Mirrors `&str::strip_prefix`
/// so callers can keep their idiomatic `if let Some(stripped) = ...`
/// shape. Returns `None` for relative paths.
pub(crate) fn strip_absolute(name: &str) -> Option<&str> {
    name.strip_prefix(ABSOLUTE_PREFIX)
}

/// Marker prefix for a splatted rescue filter (`rescue *CONST`).
/// The marked name is the CONSTANT's name, not a class name — the
/// `PushRescue` handler resolves it to an Array value and matches
/// the exception against each element. `*` can't begin a real
/// constant name, so the marker can't collide. Composes OUTSIDE
/// the absolute marker: `rescue *::Foo::BAR` encodes as
/// `*::Foo::BAR` — strip the splat first, then the absolute.
pub(crate) const SPLAT_PREFIX: &str = "*";

/// Tag a rescue-filter constant name as splatted.
pub(crate) fn tag_splat(name: String) -> String {
    let mut tagged = String::with_capacity(SPLAT_PREFIX.len() + name.len());
    tagged.push_str(SPLAT_PREFIX);
    tagged.push_str(&name);
    tagged
}

/// Strip the splat marker if present. Returns `None` for ordinary
/// (non-splat) rescue class names.
pub(crate) fn strip_splat(name: &str) -> Option<&str> {
    name.strip_prefix(SPLAT_PREFIX)
}

/// Marker prefix for a splatted rescue filter whose operand is a
/// LOCAL variable (`rescue *exp` — minitest's `assert_raises *exp`
/// idiom, where `exp` is the method's own splat-args array). The
/// marked name is the local's name; the COMPILER resolves it to a
/// slot and emits `Op::PushRescueSplatLocal`, so unlike the
/// constant-splat marker this one never reaches the runtime.
/// `&` can't begin a constant or local name in source, so the
/// marker can't collide with either of the other two.
pub(crate) const SPLAT_LOCAL_PREFIX: &str = "&";

/// Tag a rescue-filter local name as splatted-local.
pub(crate) fn tag_splat_local(name: String) -> String {
    let mut tagged = String::with_capacity(SPLAT_LOCAL_PREFIX.len() + name.len());
    tagged.push_str(SPLAT_LOCAL_PREFIX);
    tagged.push_str(&name);
    tagged
}

/// Strip the splatted-local marker if present.
pub(crate) fn strip_splat_local(name: &str) -> Option<&str> {
    name.strip_prefix(SPLAT_LOCAL_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_then_strip_roundtrip() {
        let abs = tag_absolute("Foo::Bar".to_string(), true);
        assert_eq!(abs, "::Foo::Bar");
        assert_eq!(strip_absolute(&abs), Some("Foo::Bar"));

        let rel = tag_absolute("Foo::Bar".to_string(), false);
        assert_eq!(rel, "Foo::Bar");
        assert_eq!(strip_absolute(&rel), None);
    }

    #[test]
    fn tag_absolute_false_is_identity_no_realloc() {
        // No-op path: bare relative names should not allocate.
        let s = "Foo::Bar".to_string();
        let ptr_before = s.as_ptr();
        let s = tag_absolute(s, false);
        let ptr_after = s.as_ptr();
        assert_eq!(ptr_before, ptr_after,
            "tag_absolute(_, false) must not reallocate the String");
    }

    #[test]
    fn strip_absolute_single_segment() {
        assert_eq!(strip_absolute("::TopErr"), Some("TopErr"));
        assert_eq!(strip_absolute("TopErr"), None);
    }

    #[test]
    fn splat_tag_roundtrip_and_composition() {
        let s = tag_splat("PASSTHROUGH".to_string());
        assert_eq!(s, "*PASSTHROUGH");
        assert_eq!(strip_splat(&s), Some("PASSTHROUGH"));
        assert_eq!(strip_splat("PASSTHROUGH"), None);

        // splat outside absolute: `rescue *::Foo::BAR`
        let composed = tag_splat(tag_absolute("Foo::BAR".to_string(), true));
        assert_eq!(composed, "*::Foo::BAR");
        let inner = strip_splat(&composed).unwrap();
        assert_eq!(strip_absolute(inner), Some("Foo::BAR"));
    }
}
