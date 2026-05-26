# A6d (revisited post PR #89): load msgpack-ruby's vendored
# `lib/msgpack/bigint.rb` and round-trip i64-range values via
# the proper `MessagePack::Bigint` nested-module path.
#
# Originally shipped as a Rust integration test
# (`tests/cext_msgpack_bigint.rs`) because rubyrs flattened
# nested `module Foo; module Bar; end; end` to top-level, so
# `MessagePack::Bigint` returned `nil` and the test had to
# reach the methods via top-level `Bigint`. PR #89's lexical
# constant scoping (dual-write into both bare and prefixed
# keys) closed that gap — `MessagePack::Bigint.to_msgpack_ext`
# now resolves to the same Module/method pair CRuby does, so
# this is a regular diff_cruby fixture again.
#
# Scope (Tier 1 protocol-compat per ADR 0015):
#   - Inputs MUST be in i64 range. Values beyond i64::MAX /
#     i64::MIN saturate at the rubyrs parser before bigint.rb
#     sees them; BigInt arithmetic is Tier 2 work.
#   - i64::MIN is intentionally skipped: bigint.rb's
#     `bigint = -bigint` magnitude step overflows on it (CRuby
#     promotes silently; rubyrs would saturate). Documented
#     edge case.
#
# What this proves end-to-end:
#   1. `require_relative ".../bigint.rb"` works (A5).
#   2. `MessagePack::Bigint` nested-module path resolves
#      (PR #89, supersedes the A6d Rust-test workaround).
#   3. `Integer.instance_method(:[]).arity != 1` resolves
#      without NameError (A6c — primitive class
#      `instance_method`).
#   4. `bigint[offset, length]` extracts bitfields (A6b).
#   5. `Array#pack("CL>*")` produces correct BE u32 limbs
#      and `Array#unpack("CL>*")` reverses (A6a + D9).
#   6. `<<` accumulator + `Integer#+` arithmetic on i64
#      drives `from_msgpack_ext`.

require_relative "../../examples/msgpack-cext/vendor-rb/msgpack/bigint.rb"

# Eight cases spanning sign × magnitude × i64 width.
# Per-line format matches what CRuby prints, so byte-identical
# diff doesn't depend on any external "expected" capture.
cases = [
  0,
  1,
  -1,
  255,
  2147483647,           # i32::MAX
  -2147483647,          # -(i32::MAX) — negative tag + 32-bit magnitude
  0x123456789ABCDEF0,   # two-limb 64-bit
  9223372036854775807,  # i64::MAX
  # i64::MIN skipped — see header
]

cases.each_with_index do |n, i|
  bytes = MessagePack::Bigint.to_msgpack_ext(n)
  back  = MessagePack::Bigint.from_msgpack_ext(bytes)
  puts "i=#{i} bytes=#{bytes.bytes.inspect} back=#{back} match=#{back == n}"
end
