# Tier 1 seeded `SecureRandom` shim — wraps a hidden `Random`
# instance so scripts that need byte / hex / UUID / alphanumeric
# helpers can use the same API surface CRuby exposes. ADR 0017
# row 131 puts the seeded mode in Tier 1; the cryptographic
# guarantee that CRuby's SecureRandom carries (system entropy +
# OS CSPRNG) is explicitly out of scope, traded for determinism.
# Scripts that need true crypto randomness reach for a higher-
# tier capability the embedding host injects.
#
# The hidden default is seeded to 0 at preamble load. Scripts
# reseed via `SecureRandom.seed = N` for deterministic output
# (test fixtures, regression-pinning) or accept the default
# for shape-only uses (CSRF tokens in trusted-tenant scripts,
# request IDs for logging).
#
# Loaded unconditionally as part of the Tier 1 preamble — NOT
# gated behind `--features stdlib` (that gate is for stdlib
# names we vendor at request-time via `is_stdlib_stub_name`;
# `SecureRandom` matches that whitelist too, but our preamble
# materialises the constant earlier and with real methods, so
# the stub require path's `or_insert_with` short-circuits to a
# no-op).

module SecureRandom
  # Hidden Random — class variable so subclasses (rare) share
  # the same default. Single mutable slot reseeded by
  # `SecureRandom.seed =`.
  @@rng = Random.new(0)

  def self.seed=(n)
    @@rng = Random.new(n)
    n
  end

  # `n` raw bytes. CRuby returns a binary `String`; we do too
  # (`Random#bytes` is the underlying primitive).
  def self.random_bytes(n = 16)
    @@rng.bytes(n)
  end

  # `n` random bytes formatted as 2n lowercase hex chars. CRuby
  # uses `unpack1("H*")`; rubyrs Tier 1 has `unpack` but the
  # single-element shorthand may vary, so we go through `unpack`
  # + `first`. The byte→hex mapping is identical.
  def self.hex(n = 16)
    bytes = random_bytes(n).unpack("H*")
    bytes.first
  end

  # `n` random alphanumeric chars from the [0-9A-Za-z] alphabet
  # (62 entries). Each char picks a uniform-ish slot via
  # `byte % 62` — minor bias on the tail (256 isn't a multiple
  # of 62), matching CRuby's implementation as documented in
  # SecureRandom.alphanumeric's MRI source.
  ALPHANUMERIC = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
  def self.alphanumeric(n = 16)
    out = ""
    random_bytes(n).each_byte do |b|
      out << ALPHANUMERIC[b % 62]
    end
    out
  end

  # UUID v4 shape — 36 chars: `xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`
  # where x is hex and y is one of {8,9,a,b} (variant bits). CRuby's
  # SecureRandom.uuid emits this exact shape from 16 random bytes
  # with the version + variant nibbles forced. We do the same — the
  # only divergence is the underlying entropy source (Mulberry32 vs
  # OS CSPRNG).
  def self.uuid
    raw = random_bytes(16).bytes
    # Force the version + variant nibbles via `%` rather than
    # bit-AND. The Random preamble masks 32-bit state with `%
    # 0x100000000` to stay BigInt-Phase-A-friendly (Phase A
    # didn't ship bit-AND); we use the same idiom here for
    # consistency even though these values are tiny — `raw[6] %
    # 16` is the bottom nibble, `raw[8] % 64` is the bottom 6
    # bits.
    raw[6] = (raw[6] % 16) | 0x40       # version 4 (top nibble = 4)
    raw[8] = (raw[8] % 64) | 0x80       # variant 10xxxxxx (top two bits)
    # 16 bytes → 32 lowercase hex chars. `pack("C*").unpack("H*")`
    # is the canonical Ruby byte→hex idiom; `Integer#to_s(16)` per
    # byte (now available since the Tier 1 gap-fill batch) would
    # work too but adds a `.rjust(2, "0")` step and 16 extra
    # allocations.
    hex = raw.pack("C*").unpack("H*").first
    hex[0, 8] + "-" + hex[8, 4] + "-" + hex[12, 4] + "-" + hex[16, 4] + "-" + hex[20, 12]
  end
end
