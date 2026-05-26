# Tier 1 seeded `SecureRandom`. Property-based assertions —
# Mulberry32 output doesn't match CRuby's Mersenne Twister
# bit-for-bit, so we pin SHAPE (length, character class, types)
# rather than EXACT VALUES. Both implementations agree on the
# resulting true/false answers under this fixture.
#
# Determinism via `SecureRandom.seed=` is a rubyrs-specific
# Tier 1 affordance (CRuby's SecureRandom is entropy-only and
# has no seed= surface); pinned by a Rust-side embed test
# instead so this fixture stays diff_cruby-clean.
#
# `require 'securerandom'` is a no-op on rubyrs (the preamble
# already materialised the module + methods) but CRuby needs
# the explicit require to bring the stdlib into scope.
require 'securerandom'

# Module identity + constant resolves at the top level.
puts SecureRandom.class.name        # "Module"
puts defined?(SecureRandom)         # "constant"

# `hex(n)` — 2n lowercase hex chars.
puts SecureRandom.hex(8).length     # 16
puts SecureRandom.hex(8).match?(/^[0-9a-f]+$/) == true
puts SecureRandom.hex(16).length    # 32
puts SecureRandom.hex(0).length     # 0
puts SecureRandom.hex.length        # 32 (default n=16 → 32 chars)

# `random_bytes(n)` — n bytes as a binary String.
puts SecureRandom.random_bytes(20).class.name        # "String"
puts SecureRandom.random_bytes(20).bytesize          # 20
puts SecureRandom.random_bytes.bytesize              # 16 (default)
puts SecureRandom.random_bytes(0).bytesize           # 0

# `alphanumeric(n)` — n chars from [0-9A-Za-z].
puts SecureRandom.alphanumeric(32).length            # 32
puts SecureRandom.alphanumeric(32).match?(/^[0-9A-Za-z]+$/) == true
puts SecureRandom.alphanumeric.length                # 16 (default)
puts SecureRandom.alphanumeric(0).length             # 0

# `uuid` — UUID v4 shape:
#   xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx
#   y ∈ {8, 9, a, b} (variant 10xx)
u = SecureRandom.uuid
puts u.length                       # 36
puts u.match?(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/) == true
