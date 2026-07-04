# Differential fuzz battery for the string→Integer family:
# `String#to_i(base)`, `Kernel#Integer(str, base, exception:)`,
# `String#hex`, `String#oct`, and sprintf's `%d` String coercion —
# every entry point that folds a digit string, all routed through
# the shared `str2int` scanner (the i64-wrap fix).
#
# Deterministic xorshift PRNG, so this exact case list runs
# byte-identically under CRuby (the oracle) and rubyrs. Dimensions:
# digit counts 1..80, bases {none, 0, 2, 8, 10, 16, 36, random,
# negative-for-Integer, invalid}, underscores (valid single +
# invalid double/leading/trailing), signs (incl. doubled + spaced),
# ASCII/unicode whitespace, matching AND mismatched base prefixes,
# garbage affixes, empty/sign-only/underscore-only strings, i64/u64
# boundary ±2 rendered in EVERY base 2..36 (both signs), fullwidth
# unicode digits, and `exception: false` sweeps.
#
# >50_000 generated strings; each exercises 8 entry points
# (~430k conversions total). Error results print class + message,
# so CRuby's exact ArgumentError/TypeError text (with
# inspect-formatted receivers) is part of the contract.

M = (1 << 64) - 1
$st = 0x243F6A8885A308D3
def rnd
  x = $st
  x = (x ^ (x << 13)) & M
  x ^= x >> 7
  x = (x ^ (x << 17)) & M
  $st = x
end

def pick(a)
  a[rnd % a.size]
end

def try
  yield.inspect
rescue ArgumentError, TypeError => e
  "#{e.class}: #{e.message}"
end

WS      = ["", " ", "  ", "\t", "\n", "\v", "\f", "\r", " \t\n", " ", "　"].freeze
SIGNS   = ["", "", "", "+", "-", "-", "--", "+-", "- ", "+ "].freeze
PREFIXES = ["", "", "", "0x", "0X", "0b", "0B", "0o", "0O", "0d", "0D", "0"].freeze
TAILS   = ["", "", "", "", "abc", "g", "z", "!", ".5", "e3", " 1", "_", "__2", " ", "x", "４"].freeze
DEC     = ("0".."9").to_a.freeze
HEX     = (DEC + ("a".."f").to_a + ("A".."F").to_a).freeze
B36     = (DEC + ("a".."z").to_a + ("A".."Z").to_a).freeze
ODD     = ["４", "２", "１", "٣", "_"].freeze # fullwidth + arabic-indic digits

# Random digit body. Mostly short; occasionally up to 80 digits so
# the BigInt promotion path gets constant traffic.
def body
  alphabet = case rnd % 10
             when 0..4 then DEC
             when 5..6 then HEX
             when 7..8 then B36
             else ODD
             end
  len = 1 + rnd % (rnd % 7 == 0 ? 80 : 24)
  s = +""
  len.times { s << pick(alphabet) }
  # Underscore injection: valid (between digits) or deliberately
  # broken (doubled / leading / trailing).
  case rnd % 8
  when 0
    s.insert(1 + rnd % s.size, "_") if s.size > 1
  when 1
    s.insert(1 + rnd % s.size, "__") if s.size > 1
  when 2
    s = "_" + s
  when 3
    s = s + "_"
  end
  s
end

def gen_case
  case rnd % 24
  when 0 then ""                # empty
  when 1 then pick(SIGNS)       # sign-only
  when 2 then "_"               # underscore-only
  when 3 then pick(WS)          # whitespace-only
  else
    pick(WS) + pick(SIGNS) + pick(PREFIXES) + body + pick(TAILS) + pick(WS)
  end
end

TO_I_BASES    = [0, 2, 8, 10, 16, 36].freeze
INT_BASES     = [0, -1, 2, 8, 10, 16, -16, 36, -36].freeze
INVALID_BASES = [1, 37, -37, 99].freeze

lines = 0
50_500.times do |i|
  s = gen_case
  b  = (rnd % 5 == 0) ? pick(INVALID_BASES) : (rnd % 3 == 0 ? 2 + rnd % 35 : pick(TO_I_BASES))
  ib = (rnd % 6 == 0) ? pick(INVALID_BASES) : (rnd % 3 == 0 ? 2 + rnd % 35 : pick(INT_BASES))
  out = [
    s.inspect,
    try { s.to_i },
    try { s.to_i(b) },
    try { Integer(s) },
    try { Integer(s, ib) },
    try { Integer(s, exception: false) },
    try { s.hex },
    try { s.oct },
    try { sprintf("%d", s) },
  ]
  puts "#{i} b=#{b} ib=#{ib} #{out.join(" | ")}"
  lines += 1
end

# --- exhaustive i64/u64 boundary matrix: ±2 around 2^63 and 2^64,
# rendered in every base 2..36, both signs, to_i + strict Integer ---
edges = []
[(1 << 63), (1 << 64)].each do |pivot|
  (-2..2).each { |d| edges << pivot + d }
end
(2..36).each do |base|
  edges.each do |v|
    [v, -v].each do |sv|
      s = sv.to_s(base)
      r1 = try { s.to_i(base) }
      r2 = try { Integer(s, base) }
      ok = (r1 == sv.inspect && r2 == sv.inspect) ? "ok" : "MISMATCH #{r1} #{r2}"
      puts "edge base=#{base} #{s} #{ok}"
      lines += 1
    end
  end
end

# --- exception: false sweep over every case shape once more ---
2_000.times do |i|
  s = gen_case
  ib = pick(INT_BASES)
  puts "exc #{i} #{s.inspect} #{try { Integer(s, exception: false) }} #{try { Integer(s, ib, exception: false) }}"
  lines += 1
end

puts "cases=#{lines}"
