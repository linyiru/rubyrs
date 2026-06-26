# Tier 1 seeded `Random` class. MT19937 (Mersenne Twister) implemented in pure
# Ruby, BYTE-COMPATIBLE with CRuby's `Random` — same seeding (init_genrand for a
# single-word seed, init_by_array otherwise), same tempering, same integer
# `limited_rand` rejection-bounding, and same 53-bit `genrand_real` float. State
# is 624 32-bit words in `@mt`; every arithmetic step is masked back into the
# 32-bit window with `& 0xffffffff` so the BigInt feature doesn't disturb the
# cycle.
#
# Per ADR 0017 row 131 (`Random` / `SecureRandom`): the seeded mode lives in
# Tier 1; the unseeded form (system entropy) is explicitly out of scope.
# `Random.new` therefore REQUIRES an explicit Integer seed.
#
# Matching MRI bit-for-bit (was previously an explicit non-goal under Mulberry32)
# lets minitest's `srand(seed)`-then-`shuffle` reproduce CRuby's exact test
# order — needed so order-sensitive suites (e.g. zeitwerk's) behave identically.

class Random
  MT_N = 624
  MT_M = 397
  MT_MATRIX_A = 0x9908b0df
  MT_UPPER = 0x80000000
  MT_LOWER = 0x7fffffff
  MT_MASK32 = 0xffffffff

  def initialize(seed = nil)
    if seed.nil?
      raise ArgumentError, "Tier 1 Random.new requires an explicit Integer seed"
    end
    unless seed.is_a?(Integer)
      raise TypeError, "no implicit conversion of #{seed.class} into Integer"
    end
    @seed = seed
    @mt = Array.new(MT_N, 0)
    # Convert |seed| to least-significant-word-first 32-bit limbs, like CRuby's
    # INTEGER_PACK_LSWORD_FIRST. A single limb (incl. 0) seeds via init_genrand;
    # multiple limbs go through init_by_array (after the 19650218 prelude).
    s = seed.abs
    words = []
    if s.zero?
      words = [0]
    else
      while s > 0
        words << (s & MT_MASK32)
        s >>= 32
      end
    end
    if words.length <= 1
      mt_init_genrand(words[0] || 0)
    else
      mt_init_genrand(19650218)
      mt_init_by_array(words)
    end
  end

  # Original seed passed to `new`. CRuby exposes this through `Random#seed`.
  def seed
    @seed
  end

  # Shape mirrors CRuby's `Random#rand`:
  #   - no arg  → Float in [0.0, 1.0)
  #   - Integer → Integer in 0...arg
  #   - Float   → Float in [0.0, arg)
  #   - Range   → Integer or Float depending on endpoints, honouring exclude_end?
  # Invalid args (negative, zero, wrong type) raise CRuby's ArgumentError /
  # TypeError shape.
  def rand(arg = nil)
    case arg
    when nil
      mt_genrand_real
    when Integer
      raise ArgumentError, "invalid argument" if arg <= 0
      mt_limited_rand(arg - 1)
    when Float
      raise ArgumentError, "invalid argument" if arg <= 0.0
      mt_genrand_real * arg
    when Range
      lo = arg.begin
      hi = arg.end
      raise ArgumentError, "invalid argument" if lo.nil? || hi.nil?
      if lo.is_a?(Integer) && hi.is_a?(Integer)
        hi -= 1 if arg.exclude_end?
        span = hi - lo + 1
        raise ArgumentError, "invalid argument" if span <= 0
        lo + mt_limited_rand(span - 1)
      elsif lo.is_a?(Float) || hi.is_a?(Float)
        span = hi.to_f - lo.to_f
        raise ArgumentError, "invalid argument" if span <= 0.0
        lo.to_f + mt_genrand_real * span
      else
        raise TypeError, "invalid argument type"
      end
    else
      raise TypeError, "invalid argument type"
    end
  end

  # `n` raw bytes as a binary String, 4 little-endian bytes per 32-bit draw.
  def bytes(n)
    raise ArgumentError, "negative size" if n < 0
    out = String.new
    while out.bytesize < n
      v = mt_genrand_int32
      4.times do
        break if out.bytesize >= n
        out << (v % 256).chr
        v >>= 8
      end
    end
    out
  end

  private

  # init_genrand — seed the state from a single 32-bit word (CRuby/MT reference).
  def mt_init_genrand(s)
    @mt[0] = s & MT_MASK32
    j = 1
    while j < MT_N
      prev = @mt[j - 1]
      @mt[j] = (1812433253 * (prev ^ (prev >> 30)) + j) & MT_MASK32
      j += 1
    end
    @mti = MT_N
  end

  # init_by_array — seed from a multi-word key (CRuby uses this for seeds wider
  # than 32 bits, after an init_genrand(19650218) prelude).
  def mt_init_by_array(key)
    i = 1
    j = 0
    k = MT_N > key.length ? MT_N : key.length
    while k > 0
      prev = @mt[i - 1]
      @mt[i] = ((@mt[i] ^ ((prev ^ (prev >> 30)) * 1664525)) + key[j] + j) & MT_MASK32
      i += 1
      j += 1
      if i >= MT_N
        @mt[0] = @mt[MT_N - 1]
        i = 1
      end
      j = 0 if j >= key.length
      k -= 1
    end
    k = MT_N - 1
    while k > 0
      prev = @mt[i - 1]
      @mt[i] = ((@mt[i] ^ ((prev ^ (prev >> 30)) * 1566083941)) - i) & MT_MASK32
      i += 1
      if i >= MT_N
        @mt[0] = @mt[MT_N - 1]
        i = 1
      end
      k -= 1
    end
    @mt[0] = 0x80000000
  end

  # genrand_int32 — the core MT19937 generator (regenerate the 624-word block on
  # demand, then temper).
  def mt_genrand_int32
    if @mti >= MT_N
      kk = 0
      while kk < MT_N - MT_M
        y = (@mt[kk] & MT_UPPER) | (@mt[kk + 1] & MT_LOWER)
        @mt[kk] = @mt[kk + MT_M] ^ (y >> 1) ^ ((y & 1).zero? ? 0 : MT_MATRIX_A)
        kk += 1
      end
      while kk < MT_N - 1
        y = (@mt[kk] & MT_UPPER) | (@mt[kk + 1] & MT_LOWER)
        @mt[kk] = @mt[kk + (MT_M - MT_N)] ^ (y >> 1) ^ ((y & 1).zero? ? 0 : MT_MATRIX_A)
        kk += 1
      end
      y = (@mt[MT_N - 1] & MT_UPPER) | (@mt[0] & MT_LOWER)
      @mt[MT_N - 1] = @mt[MT_M - 1] ^ (y >> 1) ^ ((y & 1).zero? ? 0 : MT_MATRIX_A)
      @mti = 0
    end
    y = @mt[@mti]
    @mti += 1
    y ^= (y >> 11)
    y ^= (y << 7) & 0x9d2c5680
    y ^= (y << 15) & 0xefc60000
    y ^= (y >> 18)
    y & MT_MASK32
  end

  # genrand_real — 53-bit resolution Float in [0.0, 1.0), CRuby's
  # int_pair_to_real for the default real interval.
  def mt_genrand_real
    a = mt_genrand_int32 >> 5
    b = mt_genrand_int32 >> 6
    (a * 67108864.0 + b) * (1.0 / 9007199254740992.0)
  end

  # Smallest 2**k - 1 mask >= x (CRuby make_mask).
  def mt_make_mask(x)
    x |= x >> 1
    x |= x >> 2
    x |= x >> 4
    x |= x >> 8
    x |= x >> 16
    x |= x >> 32
    x
  end

  # limited_rand — uniform Integer in 0..limit via rejection sampling on the
  # mask (CRuby's algorithm; word-by-word from MSW to LSW so wide limits match).
  def mt_limited_rand(limit)
    return 0 if limit.zero?
    mask = mt_make_mask(limit)
    if limit <= MT_MASK32
      loop do
        val = mt_genrand_int32 & mask
        return val if val <= limit
      end
    else
      # Limits wider than 32 bits: fill word-by-word, retry if over.
      loop do
        val = 0
        i = (limit.bit_length + 31) / 32 - 1
        over = false
        while i >= 0
          word_mask = (mask >> (i * 32)) & MT_MASK32
          if word_mask != 0
            val |= (mt_genrand_int32 & word_mask) << (i * 32)
          end
          i -= 1
        end
        return val if val <= limit
      end
    end
  end
end

# Top-level `rand` / `srand` — backed by a DETERMINISTIC default RNG. CRuby seeds
# its implicit RNG from system entropy; ADR 0017's sandbox excludes entropy, so
# rubyrs's default seeds from the constant 0 instead: every run of a script that
# never calls `srand` sees the same sequence. `srand(n)` hands control to the
# caller — exactly the knob test runners like minitest use (`srand(seed)` then
# `shuffle` for reproducible test order). With the MT19937 core above, those
# sequences and shuffles now match CRuby byte-for-byte.
def __rubyrs_default_random
  $__rubyrs_default_random ||= Random.new(0)
end

def rand(arg = nil)
  __rubyrs_default_random.rand(arg)
end

def srand(seed = 0)
  unless seed.is_a?(Integer)
    raise TypeError, "no implicit conversion of #{seed.class} into Integer"
  end
  prev = $__rubyrs_default_random_seed || 0
  $__rubyrs_default_random = Random.new(seed)
  $__rubyrs_default_random_seed = seed
  prev
end

# Array#shuffle / #shuffle! / #sample — Fisher-Yates over the default RNG (or an
# explicit `random:` source responding to `rand(n)`). With the MT19937 RNG the
# permutations now match CRuby exactly for a given seed.
class Array
  def shuffle(random: nil)
    dup.shuffle!(random: random)
  end

  def shuffle!(random: nil)
    rng = random || ($__rubyrs_default_random ||= Random.new(0))
    i = length
    while i > 1
      j = rng.rand(i)
      i -= 1
      tmp = self[i]
      self[i] = self[j]
      self[j] = tmp
    end
    self
  end

  def sample(n = nil, random: nil)
    rng = random || ($__rubyrs_default_random ||= Random.new(0))
    if n.nil?
      return nil if empty?
      self[rng.rand(length)]
    else
      raise ArgumentError, "negative sample number (#{n})" if n < 0
      shuffle(random: rng).first(n)
    end
  end
end
