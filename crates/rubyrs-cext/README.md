# rubyrs-cext

> **Opaque C ABI for hosting CRuby-shape C extensions inside
> [`rubyrs`](https://crates.io/crates/rubyrs).** Spike Level 0:
> the smallest viable surface, not a CRuby `ruby.h` clone.

This crate is the FFI seam between the rubyrs interpreter and
unmodified Ruby C extensions. It exports `rb_*` functions whose
*signatures* match CRuby's `<ruby.h>` closely enough that a C
extension written against the standard `Init_<name>` /
`rb_define_global_function` flow compiles and links — but the
underlying `VALUE` is an opaque handle into a thread-local
state table, not a tagged pointer into a real Ruby object graph.

## Status

**Experimental — spike Level 0.** Enough to compile a hello-world C
extension, register a callback, and dispatch back into it from Ruby
code running on rubyrs. Not enough to run a real production gem
like `pg` or `nokogiri` — that's a separate, much larger surface.

| What's in | What's not (yet) |
| :--- | :--- |
| `Qnil` / `Qtrue` / `Qfalse` (fixed handles 0/1/2) | Tagged-pointer `VALUE` (no `FIXNUM_P` / `RB_IMMEDIATE_P` macros) |
| `rb_str_new_cstr`, basic value materialisation | Full `String` / `Array` / `Hash` introspection API |
| `rb_define_global_function` + callback dispatch | `rb_define_method` on user classes |
| `rb_raise` / `longjmp` protection across the FFI boundary | Full exception object model |
| Thread-local `CExtState` push/pop (`enter` / `leave`) | GC-integrated handle lifetimes |

See [`docs/CEXT_SAFETY.md`](../../docs/CEXT_SAFETY.md) for the
consolidated safety contract and
[ADR 0009](../../docs/adr/0009-cext-panic-policy.md) for the
panic policy.

## How it links

`rlib`-only by design. We **do not** produce a `cdylib`:

> A `cdylib` would give us two physically distinct copies of
> `STATE` — one inside the rubyrs binary's statically-linked
> image, one inside the `.dylib` that C extensions also linked
> against. They would never see each other's `enter` / `leave`.

Instead, the rubyrs binary exports the `rb_*` symbols from its
own image (via `crates/rubyrs/build.rs`), and C extensions link
with `-undefined dynamic_lookup` (macOS) or
`--unresolved-symbols=ignore-all` (Linux). The symbols resolve
against the host process at `dlopen` time. Single image, single
`STATE`.

## Use this crate?

If you're writing Rust code, you probably don't want to depend
on this directly — it's not a general-purpose CRuby FFI crate.
Use it only if you're:

- Working on rubyrs itself, or
- Writing a C extension that needs to compile and run against
  rubyrs (in which case you don't link to this Rust crate at all
  — you `#include <ruby.h>` and let the dlopen-time symbol
  resolution wire you up).

See the parent [`rubyrs`](../rubyrs/) crate for the embeddable
interpreter that consumes this.

## License

Dual-licensed under [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
