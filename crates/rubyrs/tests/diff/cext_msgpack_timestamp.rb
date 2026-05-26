# Vendored msgpack-ruby `lib/msgpack/timestamp.rb` loads cleanly
# and produces byte-identical output to CRuby. Exercises:
#
#   - Nested-module resolution (PR #89's dual-write).
#     `MessagePack::Timestamp` resolves from outside the
#     module body.
#   - Bare `new` inside `def self.from_msgpack_ext` —
#     `new(sec, 0)` is `self.new(sec, 0)` where `self` is
#     the Timestamp class. This commit's dispatch fix is
#     load-bearing for the upstream code as-shipped (no
#     rewrites to explicit `self.new`).
#   - `Array#pack("L>")` / `Array#pack("L>2")` /
#     `Array#pack("L>q>")` BE-unsigned + signed-BE-64
#     directives (A6a + the b7f79d2 signed-int expansion).
#   - `String#unpack` with `L>` / `L>2` / `L>q>` formats
#     (same engine, reverse direction).
#
# What it proves: an unmodified upstream msgpack-ruby Ruby
# helper that's strictly Tier-1 (pure pack/unpack + nested
# module + bare new) runs without divergence. Timestamp is
# notable because ADR 0017 explicitly cites the
# `MessagePack::Timestamp`-shape (sec/nsec integer pair) as
# the supported Tier-1 way to model Time without modelling
# wall-clock semantics in core.

require_relative "../../examples/msgpack-cext/vendor-rb/msgpack/timestamp.rb"

# --- timestamp32: 4-byte payload, sec only, nsec = 0 ---
data = [100].pack("L>")
ts = MessagePack::Timestamp.from_msgpack_ext(data)
puts "ts32 sec=#{ts.sec} nsec=#{ts.nsec}"            # 100 / 0

# --- timestamp64: 8-byte payload, nsec packed with sec ---
# Encoding: top 30 bits = nsec, bottom 34 bits = sec.
# Constructed via the inverse of upstream's pack rule:
#   first u32 BE = (nsec << 2) | (sec >> 32)
#   second u32 BE = sec & 0xffffffff
sec = 7
nsec = 123_456
n_top = (nsec << 2) | (sec >> 32)
n_bot = sec & 0xffffffff
data64 = [n_top, n_bot].pack("L>2")
ts64 = MessagePack::Timestamp.from_msgpack_ext(data64)
puts "ts64 sec=#{ts64.sec} nsec=#{ts64.nsec}"        # 7 / 123456

# --- timestamp96: 12-byte payload, nsec uint32be + sec i64be ---
sec96 = -1                                            # signed
nsec96 = 987_654
data96 = [nsec96, sec96].pack("L>q>")
ts96 = MessagePack::Timestamp.from_msgpack_ext(data96)
puts "ts96 sec=#{ts96.sec} nsec=#{ts96.nsec}"        # -1 / 987654

# --- Accessor readback ---
t = MessagePack::Timestamp.new(42, 999)
puts t.sec                                            # 42
puts t.nsec                                           # 999

# --- TYPE / MAX constants reachable ---
puts MessagePack::Timestamp::TYPE                     # -1
puts MessagePack::Timestamp::TIMESTAMP32_MAX_SEC      # 4294967295
puts MessagePack::Timestamp::TIMESTAMP64_MAX_SEC      # 17179869183
