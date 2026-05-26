# Tier 1 seeded `Random` class. Mulberry32 PRNG implemented in
# pure Ruby — 32-bit state stored in `@state`, every arithmetic
# step masked back into the 32-bit window with `% 0x100000000`
# so the BigInt feature (which would otherwise promote on
# overflow) doesn't disturb the cycle.
#
# Per ADR 0017 row 131 (`Random` / `SecureRandom`): the seeded
# mode lives in Tier 1; the unseeded form (system entropy) is
# explicitly out of scope. `Random.new` therefore REQUIRES an
# explicit Integer seed — no fall-through to a default
# entropy source. Scripts that want determinism use any seed;
# scripts that need cryptographic randomness reach for a higher-
# tier capability the embedding host injects.
#
# Mulberry32 isn't Mersenne Twister; output is NOT byte-identical
# to CRuby's `Random`. Tests use property assertions (range,
# determinism, length) rather than exact-value comparison —
# the documented Tier 1 contract is "seeded, deterministic
# within a run", not "matches MRI's RNG bit-for-bit".

class Random
  def initialize(seed = nil)
    if seed.nil?
      raise ArgumentError, "Tier 1 Random.new requires an explicit Integer seed"
    end
    unless seed.is_a?(Integer)
      raise TypeError, "no implicit conversion of #{seed.class} into Integer"
    end
    @seed = seed
    @state = seed % 0x100000000
  end

  # Original seed passed to `new`. CRuby exposes this through
  # `Random#seed` — useful for snapshot-style determinism checks.
  def seed
    @seed
  end

  # Shape mirrors CRuby's `Random#rand`:
  #   - no arg  → Float in [0.0, 1.0)
  #   - Integer → Integer in 0...arg
  #   - Float   → Float in [0.0, arg)
  #   - Range   → Integer or Float depending on endpoints, inside
  #     the range honouring exclude_end?
  # Invalid args (negative, zero, wrong type) raise the same
  # ArgumentError / TypeError shape CRuby raises.
  def rand(arg = nil)
    n = next_u32
    case arg
    when nil
      u32_to_unit_float(n)
    when Integer
      raise ArgumentError, "invalid argument" if arg <= 0
      n % arg
    when Float
      raise ArgumentError, "invalid argument" if arg <= 0.0
      u32_to_unit_float(n) * arg
    when Range
      lo = arg.begin
      hi = arg.end
      raise ArgumentError, "invalid argument" if lo.nil? || hi.nil?
      if lo.is_a?(Integer) && hi.is_a?(Integer)
        hi -= 1 if arg.exclude_end?
        span = hi - lo + 1
        raise ArgumentError, "invalid argument" if span <= 0
        lo + (n % span)
      elsif lo.is_a?(Float) || hi.is_a?(Float)
        span = hi.to_f - lo.to_f
        raise ArgumentError, "invalid argument" if span <= 0.0
        lo.to_f + u32_to_unit_float(n) * span
      else
        raise TypeError, "invalid argument type"
      end
    else
      raise TypeError, "invalid argument type"
    end
  end

  # `n` raw bytes as a binary String. Each Mulberry32 step emits
  # 32 bits; emit 4 bytes per step (little-endian) until we have
  # `n` total. The trailing partial chunk is truncated as needed.
  # Built via Array#pack("C*") because rubyrs Tier 1 doesn't yet
  # carry `Integer#chr` and `String.new` returns a generic Object
  # rather than a `Value::Str` — both blockers for the natural
  # "build a String byte-by-byte" loop.
  def bytes(n)
    raise ArgumentError, "negative size" if n < 0
    out_bytes = []
    while out_bytes.length < n
      v = next_u32
      4.times do
        break if out_bytes.length >= n
        out_bytes << (v % 256)
        v >>= 8
      end
    end
    out_bytes.pack("C*")
  end

  private

  # Mulberry32 step — 32-bit state, 32-bit output. Every
  # intermediate masked into the 32-bit window so the BigInt
  # feature doesn't promote when the multiply overflows.
  def next_u32
    @state = (@state + 0x6D2B79F5) % 0x100000000
    z = @state
    z = ((z ^ (z >> 15)) * (z | 1)) % 0x100000000
    inner = ((z ^ (z >> 7)) * (z | 61)) % 0x100000000
    added = (z + inner) % 0x100000000
    z = (z ^ added) % 0x100000000
    (z ^ (z >> 14)) % 0x100000000
  end

  def u32_to_unit_float(n)
    # 2 ** 32 = 4294967296 — divide so the result is in [0.0, 1.0).
    n.to_f / 4294967296.0
  end
end
