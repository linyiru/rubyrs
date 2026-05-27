//! `eql?` / `hash` / `equal?` semantics across the numeric
//! tower + the Object fallback. Covers four overlapping
//! concerns that share the same equality contract:
//!
//!   1. **`equal?` is identity**, never value equality.
//!      Critical for BigInt where `Value::BigInt(id1) !=
//!      Value::BigInt(id2)` is the only way to distinguish
//!      "two BigInts at different ObjIds with the same
//!      magnitude" from "one BigInt referenced twice".
//!   2. **`eql?` is type-strict equality**. `1.eql?(1.0)` is
//!      false (Integer vs Float). The contract Hash keys
//!      use; covered for all three numeric types.
//!   3. **`hash` is within-process stable** and consistent
//!      with `eql?` (the Hash invariant: `a.eql?(b) ⇒
//!      a.hash == b.hash`). Cross-process stability is NOT
//!      promised — host code that needs persistent hashes
//!      must use `Digest::SHA*` etc., not Ruby `#hash`.
//!   4. **Universal `eql?` fallback for non-numeric receivers**
//!      delegates to the same `==` path used by `Object#==`,
//!      so user classes inherit a sensible default without
//!      having to override `eql?` separately.
//!
//! The BigInt-as-Hash-key tests cross with `embed/numeric.rs`'s
//! arithmetic surface; they live here because the load-bearing
//! assertion is about the hash/eql? interaction, not about
//! arithmetic correctness.

use super::SharedBuf;

#[cfg(feature = "bignum")]
#[test]
fn bigint_works_as_hash_key_across_allocation_and_gc() {
    // Phase B.7 contract: the Hash collection's internal key
    // lookup uses `ruby_eq`, which for BigInt does value equality
    // via num_bigint. Two separately-allocated BigInts with the
    // same magnitude must therefore behave as the same key —
    // covering insert / lookup / size accounting / collision
    // semantics, plus survival across a GC stress that reallocates
    // every intermediate.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Insert via one allocation, look up via a fresh
        // allocation of the same magnitude — must hit.
        // Inserting the same key value a second time must NOT
        // grow the hash (overwrite, not duplicate).
        // Different-magnitude BigInts → distinct keys.
        // Mixed-magnitude paths (`2**63 * 2` and `2**64`) compute
        // the same value via different code paths and must hit
        // the same slot.
        // GC stress: alloc enough Strings to push a mark-sweep
        // cycle, then re-look-up the BigInt key — must still hit.
        "h = {}\n\
         h[2 ** 100] = :first\n\
         puts h[2 ** 100]\n\
         puts h.size\n\
         h[2 ** 100] = :second\n\
         puts h.size\n\
         puts h[2 ** 100]\n\
         h[2 ** 64] = :sixty_four\n\
         puts h[2 ** 63 * 2]\n\
         puts h.size\n\
         h[2 ** 200] = :two_hundred\n\
         puts h[2 ** 200]\n\
         puts h.size\n\
         # GC stress between insert and lookup\n\
         1000.times { |i| _ = \"alloc#{i}\".dup }\n\
         puts h[2 ** 100]\n\
         puts h[2 ** 200]",
        "bigint_hash_keys.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "first");      // lookup via separate alloc
    assert_eq!(lines[1], "1");          // single key
    assert_eq!(lines[2], "1");          // overwrite, not grow
    assert_eq!(lines[3], "second");     // value updated
    assert_eq!(lines[4], "sixty_four"); // 2^63*2 finds 2^64
    assert_eq!(lines[5], "2");          // 2^100 + 2^64
    assert_eq!(lines[6], "two_hundred");
    assert_eq!(lines[7], "3");
    assert_eq!(lines[8], "second");     // last write was :second;
                                        // survives 1000 String allocs
    assert_eq!(lines[9], "two_hundred");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_hash_equality_is_order_insensitive_with_bigint_keys() {
    // Phase B.7: `Hash#==` does order-insensitive comparison via
    // ruby_eq on both keys AND values, so two hashes built in
    // different orders with the same {BigInt → Value} mapping
    // must compare equal. Pre-existing behavior; pin it so the
    // BigInt-key path stays correct as the collection evolves.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "h1 = {}\n\
         h1[2 ** 100] = :a\n\
         h1[2 ** 200] = :b\n\
         h2 = {}\n\
         h2[2 ** 200] = :b\n\
         h2[2 ** 100] = :a\n\
         puts h1 == h2\n\
         # Differing values on equal keys → not equal\n\
         h3 = {}\n\
         h3[2 ** 100] = :a\n\
         h3[2 ** 200] = :different\n\
         puts h1 == h3\n\
         # Differing keys → not equal\n\
         h4 = {}\n\
         h4[2 ** 100] = :a\n\
         h4[2 ** 201] = :b\n\
         puts h1 == h4",
        "bigint_hash_eq.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "true");   // same mapping, different order
    assert_eq!(lines[1], "false");  // differing values
    assert_eq!(lines[2], "false");  // differing keys
}

#[cfg(feature = "bignum")]
#[test]
fn array_include_p_handles_bigint_value_equality() {
    // Phase B.7: `Array#include?(needle)` uses ruby_eq, which
    // for BigInt does value equality. A `needle` allocated
    // separately from the array's stored BigInt must still hit.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "arr = [2 ** 100, 5, 2 ** 64]\n\
         puts arr.include?(2 ** 100)\n\
         puts arr.include?(2 ** 64)\n\
         puts arr.include?(2 ** 63 * 2)\n\
         puts arr.include?(2 ** 101)\n\
         puts arr.include?(5)\n\
         # uniq dedups via ==\n\
         puts [2 ** 100, 2 ** 100, 2 ** 100].uniq.size",
        "bigint_array_include.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "true");
    assert_eq!(lines[1], "true");
    assert_eq!(lines[2], "true");   // 2^63*2 == 2^64 via different path
    assert_eq!(lines[3], "false");
    assert_eq!(lines[4], "true");
    assert_eq!(lines[5], "1");      // all three BigInts coalesce
}

#[cfg(feature = "bignum")]
#[test]
fn integer_and_float_hash_pins_cross_rustc_stable_literal_values() {
    // Regression for the DefaultHasher cross-rustc stability gap.
    // Pre-fix `Integer#hash` / `Float#hash` used stdlib's
    // DefaultHasher, whose algorithm is documented as 'subject
    // to change' — the absolute u64 it returns for a given input
    // is allowed to differ between rustc versions. We swapped to
    // FNV-1a 64-bit (numeric.rs::fnv1a_64), whose constants are
    // fixed forever, so the digest is reproducible regardless
    // of toolchain version.
    //
    // Pin a handful of literal hash values for the canonical
    // inputs. If a future maintainer accidentally switches back
    // to a non-stable hasher, these assertions break before
    // anyone notices in production.
    //
    // Values computed once (rustc 1.x, FNV-1a constants per
    // <http://www.isthe.com/chongo/tech/comp/fnv/>) and locked
    // in here. The whole point is that they should NOT change
    // even when rustc updates.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 0.hash\n\
         puts 1.hash\n\
         puts 5.hash\n\
         puts 5.0.hash\n\
         puts (2 ** 100).hash",
        "hash_literals.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    // Literal values from FNV-1a('I' + i64_le_bytes) /
    // FNV-1a('F' + f64_bits_le_bytes) / FNV-1a('I' +
    // bigint_signed_bytes_le). Captured 2026-05-26.
    assert_eq!(lines[0], "-3998581643000780540");   // 0.hash
    assert_eq!(lines[1], "-1766266236033191131");   // 1.hash
    assert_eq!(lines[2], "7751216209806002849");    // 5.hash
    assert_eq!(lines[3], "-7377979328275632211");   // 5.0.hash
    assert_eq!(lines[4], "-833697570399297604");    // (2**100).hash
}

#[cfg(feature = "bignum")]
#[test]
fn integer_hash_is_within_process_stable_and_distinguishes_value() {
    // Phase B.7: `Integer#hash` returns a within-process-stable
    // i64 that satisfies `a.eql?(b) ⇒ a.hash == b.hash`. Pre-fix
    // every Integer receiver raised NoMethodError on `.hash`.
    //
    // The Hash collection itself uses linear scan via ruby_eq, so
    // this method isn't on the internal lookup path — it exists
    // for the user-facing protocol (pure-Ruby code calling
    // `n.hash` for its own bookkeeping). Stability is per-process
    // (DefaultHasher), matching CRuby's per-VM-seeded behaviour.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Same value → same hash (the key invariant).
        // Different value → almost-certainly different hash.
        // Sign matters: `n` and `-n` distinct hashes.
        // Cross-allocation BigInt stability.
        "puts 5.hash == 5.hash\n\
         puts 5.hash == 6.hash\n\
         puts (2 ** 100).hash == (2 ** 100).hash\n\
         puts (2 ** 100).hash == (2 ** 100 + 1).hash\n\
         puts (2 ** 100).hash == (-(2 ** 100)).hash\n\
         puts 5.hash.class.name\n\
         puts (2 ** 100).hash.class.name\n\
         puts 5.respond_to?(:hash)\n\
         puts (2 ** 100).respond_to?(:hash)",
        "integer_hash.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot().trim(),
        "true\nfalse\ntrue\nfalse\nfalse\nInteger\nInteger\ntrue\ntrue"
    );
}

#[cfg(feature = "bignum")]
#[test]
fn integer_eql_q_is_type_strict_equality() {
    // Phase B.7: `Integer#eql?` is value equality restricted to
    // matching numeric class. CRuby uses this (not `==`) for Hash
    // key matching at the language level, so it must distinguish
    // `5 == 5.0` (true) from `5.eql?(5.0)` (false). Pre-fix
    // rubyrs raised NoMethodError on every `Integer#eql?` call.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Int receiver:
        //   eql?(Int_same) → true
        //   eql?(Int_diff) → false
        //   eql?(Float) → false (type strict, even when values match)
        //   eql?(BigInt) → false (canonical invariant)
        //   eql?(String) → false
        // BigInt receiver:
        //   eql?(BigInt_same_value) → true (separate allocs OK)
        //   eql?(BigInt_diff) → false
        //   eql?(Int) → false (canonical invariant)
        //   eql?(Float) → false (type strict)
        // respond_to? whitelist covers both receivers.
        "puts 5.eql?(5)\n\
         puts 5.eql?(6)\n\
         puts 5.eql?(5.0)\n\
         puts 5.eql?(2 ** 100)\n\
         puts 5.eql?(\"5\")\n\
         puts (2 ** 100).eql?(2 ** 100)\n\
         puts (2 ** 100).eql?(2 ** 100 + 1)\n\
         puts (2 ** 100).eql?(5)\n\
         puts (2 ** 100).eql?(2.0)\n\
         puts 5.respond_to?(:eql?)\n\
         puts (2 ** 100).respond_to?(:eql?)",
        "integer_eql.rb",
    ).expect("eval");
    let out = buf.snapshot();
    assert_eq!(
        out.trim(),
        "true\nfalse\nfalse\nfalse\nfalse\ntrue\nfalse\nfalse\nfalse\ntrue\ntrue"
    );
}

#[test]
fn eql_q_and_hash_raise_argumenterror_on_wrong_arity() {
    // Phase B.7 review: pre-fix wrong-arity calls on eql?/hash
    // bypassed the exact-arity per-type arms and surfaced as
    // NoMethodError instead of CRuby's
    // ArgumentError. User code's `rescue ArgumentError` keys on
    // the error class, so the divergence is observable.
    //
    // Universal `eql?` interceptor raises for any non-1 arg
    // count. `hash` arity guard fires only for receivers that
    // actually support hash (gated by responds_to) so unrelated
    // `obj.hash(:x)` for obj without hash still surfaces as
    // NoMethodError per CRuby.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected) in [
        ("5.eql?(1, 2)",          "wrong number of arguments (given 2, expected 1)"),
        ("5.hash(:x)",            "wrong number of arguments (given 1, expected 0)"),
        ("5.0.eql?(1, 2)",        "wrong number of arguments (given 2, expected 1)"),
        ("5.0.hash(:x)",          "wrong number of arguments (given 1, expected 0)"),
        ("(2 ** 100).hash(:x)",   "wrong number of arguments (given 1, expected 0)"),
        ("nil.eql?(1, 2)",        "wrong number of arguments (given 2, expected 1)"),
        ("\"a\".eql?(1, 2)",      "wrong number of arguments (given 2, expected 1)"),
    ] {
        let err = rt.eval(script, "arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "ArgumentError", "for {:?}", script);
                assert_eq!(message, expected, "for {:?}", script);
            }
            other => panic!("expected Uncaught ArgumentError for {:?}, got {:?}", script, other),
        }
    }
}

#[test]
fn universal_eql_q_delegates_to_ruby_eq_for_non_numeric_receivers() {
    // Phase B.7 review: pre-fix nil/Sym/Bool/String/Array/Hash/
    // arbitrary-Object all raised NoMethodError on `.eql?(x)`
    // because only Integer (+ Float in this PR) had per-type
    // arms. CRuby's Kernel#eql? defaults to identity for user
    // objects, but Array/Hash/String override it to value
    // equality.
    //
    // Add a universal dispatch interceptor that fires AFTER
    // primitive_call (so per-type type-strict numeric arms still
    // win) and delegates to `ruby_eq`. This matches CRuby for:
    //  - immediates (Sym/Bool/Nil) — identity ≡ value
    //  - String — value equality (via ruby_eq's Str arm)
    //  - Array/Hash/Range — value equality (recursive ruby_eq)
    //  - heap-allocated Objects/Methods/Procs — ObjId identity
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Immediates: identity = value.
        // String: value equality.
        // Arrays / Hashes: value equality across allocations.
        // Cross-type: false.
        // respond_to gated universally.
        "puts nil.eql?(nil)\n\
         puts nil.eql?(false)\n\
         puts :sym.eql?(:sym)\n\
         puts :sym.eql?(:other)\n\
         puts \"a\".eql?(\"a\")\n\
         puts \"a\".eql?(\"b\")\n\
         puts true.eql?(true)\n\
         puts true.eql?(false)\n\
         puts [1, 2].eql?([1, 2])\n\
         puts [1, 2].eql?([1, 3])\n\
         puts({a: 1}.eql?({a: 1}))\n\
         puts({a: 1}.eql?({a: 2}))\n\
         puts nil.respond_to?(:eql?)\n\
         puts :sym.respond_to?(:eql?)\n\
         puts \"x\".respond_to?(:eql?)\n\
         puts [].respond_to?(:eql?)",
        "universal_eql.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot().trim(),
        "true\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue"
    );
}

#[test]
fn float_eql_and_hash_are_type_strict_siblings_to_integer() {
    // Phase B.7 review: shipping `eql?`/`hash` only on Integer
    // made the canonical `5.eql?(5.0) == false` case
    // unexercisable from the Float side. Add the sibling methods
    // with a distinct hash tag so `5.hash != 5.0.hash` —
    // required by the `a.eql?(b) ⇒ a.hash == b.hash` invariant.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 5.0.eql?(5.0)\n\
         puts 5.0.eql?(5)\n\
         puts 5.eql?(5.0)\n\
         puts 5.0.eql?(6.0)\n\
         puts 5.0.eql?(\"5\")\n\
         puts 5.0.hash == 5.0.hash\n\
         puts 5.0.hash == 5.hash\n\
         puts 5.0.respond_to?(:eql?)\n\
         puts 5.0.respond_to?(:hash)",
        "float_eql_hash.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot().trim(),
        "true\nfalse\nfalse\nfalse\nfalse\ntrue\nfalse\ntrue\ntrue"
    );
}

#[test]
fn equal_q_handles_sibling_heap_variants_via_identity() {
    // Phase B.7 drive-by: `Object#equal?` mirrored its BigInt arm
    // pattern for the four other heap-allocated variants that
    // previously fell through to ruby_eq's `_ => false` default
    // and reported `false` even for self-comparison.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "m = 5.method(:succ)\n\
         puts m.equal?(m)\n\
         um = Integer.instance_method(:succ)\n\
         puts um.equal?(um)\n\
         c = proc { |a, b| a + b }.curry\n\
         puts c.equal?(c)\n\
         r = /x/\n\
         puts r.equal?(r)",
        "equal_sibling.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot().trim(), "true\ntrue\ntrue\ntrue");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_equal_q_is_object_identity_not_value_equality() {
    // Phase B.7: `Object#equal?` is BasicObject identity, not
    // value equality. For heap-managed types (Array, Hash, Str,
    // BigInt) two separately-allocated objects with identical
    // value must NOT be `equal?`. Pre-fix BigInt fell through
    // to ruby_eq's value-equality default and `(2**64).equal?(2**64)`
    // wrongly returned true.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Two separate allocs, same value → distinct objects.
        // `a.equal?(a)` is always true (same alloc).
        // `==` (value equality) is still true.
        "a = 2 ** 64\n\
         b = 2 ** 64\n\
         puts a.equal?(b)\n\
         puts a.equal?(a)\n\
         puts (2 ** 64).equal?(2 ** 64)\n\
         puts a == b",
        "bigint_equal.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "false");  // separate allocs
    assert_eq!(lines[1], "true");   // same alloc
    assert_eq!(lines[2], "false");  // separate literals
    assert_eq!(lines[3], "true");   // value equality unchanged
}

