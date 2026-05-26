# String#unpack1(format) — like #unpack but returns just the
# first directive's result. Idiomatic when a binary-protocol
# parser knows the format produces one value and wants to
# skip `.unpack(...).first`. Real-world callers:
# msgpack-ruby's per-frame header reads, ad-hoc binary
# parsers.
#
# Scope: 1-arg form only — the `offset:` kwarg added in
# Ruby 3.1 is not implemented (rare; SUBSET-documented).
#
# Fixture coverage is limited to directives the existing
# pack/unpack engine already supports (N/n/C/a) and inputs
# that survive the string lexer + inspect path. Separate
# gaps NOT in scope here:
#   - `H*` (hex string), `l<` (signed LE i32) — pack/unpack
#     directive coverage
#   - high-byte literals (`\xFF`-class) — string lexer hits
#     UTF-8 substitution
#   - `\x00`-in-string inspect — rubyrs emits `\0`, CRuby
#     uses `\x00` (String#inspect formatting divergence)

# Single big-endian u32 — the natural shape (msgpack uses
# this for its 32-bit length headers).
puts "\x12\x34\x56\x78".unpack1("N")            # 305419896

# Single big-endian u16.
puts "\x12\x34".unpack1("n")                    # 4660

# String with width: `a5` reads exactly 5 bytes as a binary
# string. unpack1 returns the String, not an Array.
puts "hello world".unpack1("a5").inspect        # "hello"

# `C` — first byte as an unsigned 8-bit integer.
puts "ABC".unpack1("C")                         # 65 — ord('A')

# Empty input → nil (engine produces no values).
puts "".unpack1("C").inspect                    # nil

# Round-trip parity with unpack(...).first — the docs'
# definition of unpack1's semantics.
data = "\x12\x34\x56\x78"
puts data.unpack1("N") == data.unpack("N").first  # true
