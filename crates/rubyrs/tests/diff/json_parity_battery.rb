# JSON byte-parity battery — the probe corpus from the 2026-07 _json_native
# performance/correctness pass, pinned as a permanent fixture. Exercises the
# surfaces that were found divergent (and fixed) against CRuby 3.4.1 +
# json 2.20.0:
#   - fpconv float emission (JSON.generate does NOT use Float#to_s)
#   - exact bigint parse (serde visit_f64 silently lossy before)
#   - float-overflow literals (1e999 -> Infinity)
#   - frozen + fstring-deduped object keys, duplicate-key last-wins
#   - parse/generate nesting limits and their exact messages
#   - invalid-UTF-8 generate errors (class + message)
#   - non-String hash-key stringification (Float keys use Float#to_s!)
#
# Registered twice in diff_cruby.rs: once on the native accelerator and once
# with RUBYRS_JSON_NO_NATIVE=1 (pure canon) — CRuby is the oracle for both,
# so accelerator and canon cannot drift from CRuby or from each other.
#
# Known exclusions (documented divergences, NOT tested here):
#   - invalid-UTF-8 bytes in PARSE INPUT: CRuby passes raw bytes through
#     string values; rubyrs (canon chars-walk) U+FFFD-replaces them.
#   - cross-parse `.equal?` key sharing holds on both (CRuby fstring table /
#     rubyrs capped key cache) but the rubyrs cache is capped at 8192 texts,
#     so it is not asserted beyond one document here.
#   - string-error MESSAGES (control chars, bad \u escapes, surrogates)
#     are asserted class-only; number-error messages are asserted in full
#     (byte-based columns + 32-BYTE fragment cap, matching CRuby).
#
# Perf note (documented in bench/json_bench_results.md): documents with
# bare >=19-digit INTEGER literals decline whole to the pure canon —
# values exact, canon speed. Long digit runs inside STRINGS stay native.

require "json"

def try(label)
  v = yield
  puts "#{label} => #{v.inspect}"
rescue => e
  puts "#{label} !! #{e.class}: #{e.message}"
end

puts "== tiny + empty =="
puts JSON.generate({})
puts JSON.generate([])
p JSON.parse("{}")
p JSON.parse("[]")
p JSON.parse("null")
p JSON.parse("true")
p JSON.parse("42")
p JSON.parse("\"s\"")
p JSON.parse("[[],{},[{}],[[]]]")

puts "== float generate (fpconv corpus) =="
[0.0, -0.0, 1.0, -1.0, 0.5, 0.1, 2.0 / 3.0, 100.0, 12345.0, 3.14159,
 1e14, 99999999999999.0, 999999999999999.0, 1e15, 1.5e15,
 9999999999999998.0, 1e16, 1.5e16, 1e17, 1e18, 1e20, -1e20,
 123456789012345.6, 1234567890123456.8, 123456789012345678.0,
 12345678901234567890.0, 1e-4, 1e-5, 1e-6, 1e-7, 1e-8, 1.5e-5, 1.5e-6,
 1.5e-7, -1.5e-7, 1.23456789e-5, 0.00012345678901234567,
 1.2345678901234567e-7, 5e-324, 2.2250738585072014e-308,
 1.7976931348623157e308].each do |f|
  puts JSON.generate([f])
end

puts "== float parse fidelity (bits) =="
["2.2250738585072011e-308", "0.1", "2.2250738585072014e-308",
 "1.00000000000000011102230246251565404236316680908203125",
 "5e-324", "4.9406564584124654e-324", "1e308", "17976931348623157e292",
 "0.30000000000000004", "123.456e78"].each do |s|
  f = JSON.parse("[#{s}]")[0]
  puts "#{s} bits=#{[f].pack('G').unpack1('Q>').to_s(16)}"
end

puts "== float round trip =="
[1.5, 0.1, 1e15, 1.5e-5, 2.0 / 3.0].each do |f|
  puts (JSON.parse(JSON.generate([f]))[0] == f).inspect
end

puts "== non-finite =="
try("nan") { JSON.generate([Float::NAN]) }
try("inf") { JSON.generate([Float::INFINITY]) }
try("ninf") { JSON.generate([-Float::INFINITY]) }
try("dump-nonfinite") { JSON.dump([Float::NAN, Float::INFINITY, -Float::INFINITY]) }
p JSON.parse!("[NaN]")[0].nan?
p JSON.parse!("[Infinity,-Infinity]")

puts "== big integers =="
try("big") { JSON.parse('{"n": 123456789012345678901234567890}')["n"] }
try("u64") { JSON.parse("[9223372036854775808]")[0] }
try("negbig") { JSON.parse("[-9223372036854775809]")[0] }
try("i64max") { JSON.parse("[9223372036854775807]")[0] }
try("i64min") { JSON.parse("[-9223372036854775808]")[0] }
try("18dig") { JSON.parse("[123456789012345678]")[0] }
try("bigrt") { JSON.generate(JSON.parse("[123456789012345678901234567890]")) }
try("biggen") { JSON.generate([10**29 + 23456789012345678901234567890]) }
try("bigkey") { JSON.generate({ 10**25 => 1 }) }
try("bigcls") { JSON.parse("[123456789012345678901234567890]")[0].class }
try("ovf1") { JSON.parse("[1e999]")[0] }
try("ovf2") { JSON.parse("[-1e999]")[0] }
try("ovf3") { JSON.parse("[1e-999]")[0] }

puts "== unicode + escapes =="
p JSON.parse('"é € 中"')
p JSON.parse('"😀"')
p JSON.parse('"A\u0009B\u000aC\u0000D"')
p JSON.parse('"\" \\\\ \/ \b \f \n \r \t"')
p JSON.parse("\"é € 中 \u{1F600}\"")
puts JSON.generate(["\u0000\b\f\n\r\t\"\\"]).inspect
puts JSON.generate(["é € 中 \u{1F600}"])
puts JSON.generate(["a/b"])
s = JSON.generate(["\u0000\u0001\u001f"])
p s
p JSON.parse(s)

puts "== deep nesting =="
try("p100") { JSON.parse("[" * 100 + "]" * 100) && "ok" }
try("p101") { JSON.parse("[" * 101 + "]" * 101) && "ok" }
try("p102") { JSON.parse("[" * 102 + "]" * 102) && "ok" }
try("p150") { JSON.parse("[" * 150 + "]" * 150) && "ok" }
try("pmn5") { JSON.parse("[" * 7 + "]" * 7, max_nesting: 5) && "ok" }
def deep_arr(n)
  a = []
  cur = a
  (n - 1).times { x = []; cur << x; cur = x }
  a
end
try("g100") { JSON.generate(deep_arr(100)).length }
try("g101") { JSON.generate(deep_arr(101)) }
try("g150") { JSON.generate(deep_arr(150)) }
try("obj-nest") { JSON.parse(('{"k":' * 102) + "1" + ("}" * 102)) && "ok" }

puts "== duplicate keys (last-wins) =="
p JSON.parse('{"a":1,"a":2}')
p JSON.parse('{"a":1,"b":2,"a":3}')
p JSON.parse('{"a":1,"c":{"a":5},"a":9}')
p JSON.parse('{"a":1,"c":{"a":5,"a":6},"a":9,"b":2,"a":10}')
p JSON.parse('{"a":1,"a":2}', symbolize_names: true)

puts "== key frozen-ness + sharing =="
h = JSON.parse('{"key":1}')
k = h.keys[0]
puts "frozen? #{k.frozen?}"
puts "enc #{k.encoding}"
two = JSON.parse('[{"share":1},{"share":2}]')
puts "shared? #{two[0].keys[0].equal?(two[1].keys[0])}"
begin
  k << "x"
rescue => e
  puts "mutate: #{e.class}"
end

puts "== symbolize_names =="
p JSON.parse('{"name":"ada","tags":["x"],"n":{"a":1}}', symbolize_names: true)
p JSON.parse('[{"k":1},{"k":2}]', symbolize_names: true)

puts "== result shapes =="
g = JSON.generate({ "a" => 1 })
puts "gen enc=#{g.encoding} frozen=#{g.frozen?}"
ps = JSON.parse('["hello"]')[0]
puts "parse-str enc=#{ps.encoding} frozen=#{ps.frozen?}"

puts "== hash key stringification =="
puts JSON.generate({ 1.5 => 1 })
puts JSON.generate({ 1.0e-5 => 1 })
puts JSON.generate({ 1e16 => 1 })
puts JSON.generate({ nil => 1, true => 2, false => 3 })
puts JSON.generate({ 42 => 1, -7 => 2 })
puts JSON.generate({ sym: 1, "str" => 2 })

puts "== invalid UTF-8 generate =="
try("bin-inv") { JSON.generate(["\xff".b]) }
try("utf8-inv") { JSON.generate(["\xff"]) }
try("bin-mixed") { JSON.generate(["caf\xC3\xA9\xFF".b]) }
try("bin-trunc") { JSON.generate(["ok\xE2\x82".b]) }
try("utf8-bad-cont") { JSON.generate(["\xC3\x28"]) }
try("bin-ascii") { JSON.generate(["plain".b]) }

puts "== options still work (canon paths) =="
puts JSON.pretty_generate({ "a" => 1, "nested" => { "b" => [1, 2] }, "arr" => [] })
p JSON.parse('{"a":1}', max_nesting: 3)
puts JSON.dump([1.5, "x"])
puts [1, 2].to_json
puts({ "k" => nil }.to_json)
puts 1.5e-5.to_json
puts "s".to_json

puts "== nesting rule: non-empty entry (verifier item 3) =="
try("e100")  { JSON.parse("[" * 100 + "]" * 100) && "ok" }
try("e101")  { JSON.parse("[" * 101 + "]" * 101) && "ok" }
try("e102")  { JSON.parse("[" * 102 + "]" * 102) && "ok" }
try("n99")   { JSON.parse("[" * 99  + "1" + "]" * 99) && "ok" }
try("n100")  { JSON.parse("[" * 100 + "1" + "]" * 100) && "ok" }
try("n101")  { JSON.parse("[" * 101 + "1" + "]" * 101) && "ok" }
try("o100")  { JSON.parse(('{"k":' * 100) + "1" + ("}" * 100)) && "ok" }
try("o101")  { JSON.parse(('{"k":' * 101) + "1" + ("}" * 101)) && "ok" }
try("o101e") { JSON.parse(('{"k":' * 100) + "{}" + ("}" * 100)) && "ok" }
try("o102e") { JSON.parse(('{"k":' * 101) + "{}" + ("}" * 101)) && "ok" }
try("m2a")   { JSON.parse("[[1]]", max_nesting: 2) && "ok" }
try("m2b")   { JSON.parse("[[[1]]]", max_nesting: 2) && "ok" }
try("m2c")   { JSON.parse("[[[]]]", max_nesting: 2) && "ok" }
try("m2d")   { JSON.parse("[[[[]]]]", max_nesting: 2) && "ok" }
try("mix")   { JSON.parse("[" * 50 + '{"a":' + "[" * 51 + "1" + "]" * 51 + "}" + "]" * 50) && "ok" }
def deep_full(n)
  a = [1]
  (n - 1).times { a = [a] }
  a
end
try("gn100") { JSON.generate(deep_full(100)).length }
try("gn101") { JSON.generate(deep_full(101)) }
try("gmn5a") { JSON.generate(deep_full(5), max_nesting: 5).length }
try("gmn5b") { JSON.generate(deep_full(6), max_nesting: 5) }

puts "== strict number grammar (verifier item 2) =="
["[00000000000000000012]", "[012]", "[1234567890123456789.]", "[1.]",
 "[1234567890123456789e]", "[1e]", "[1e+]", "[-]", "[1.e5]", "[01.5]",
 "[-01]", "[0123456789012345678901234]", "[-0]", "[0.0e5]"].each do |s|
  try("num #{s}") { JSON.parse(s) }
end
try("snip") { JSON.parse("[012, " + '"x" ,' * 30 + " 1]") }
try("snipline") { JSON.parse("[1,\n 012]") }
# CRuby renders error columns and the 32-char fragment cap in BYTES,
# not characters (probed: multibyte before the bad token shifts the
# column; a multibyte char ending exactly at the cap is dropped whole,
# one cut at the cap loses its partial bytes).
try("mb-col") { JSON.parse('["\u00e9\u00e9",01]') }
try("mb-line") { JSON.parse("[\"x\",\n \"\u00e9\u00e9\", 01]") }
try("mb-cap29") { JSON.parse("[01234567890123456789012345678\u00e945678]") }
try("mb-cap30") { JSON.parse("[012345678901234567890123456789\u00e945678]") }
try("mb-cap31") { JSON.parse("[0123456789012345678901234567890\u00e945678]") }
try("mb-wsstrip") { JSON.parse("[01\u00e9 ]") }
try("mb-eof") { JSON.parse("[01\u00e9") }
try("mb-emoji-cap") { JSON.parse("[0123456789012345678901234567\u{1F600}8]") }
try("eofnum2") { JSON.parse("12e") }

puts "== exponent saturation (verifier item 5) =="
["[1e000000000000000009]", "[1e0000000000000000009]", "[1e00000000000000000009]",
 "[1e-00000000000000000009]", "[-1e00000000000000000009]", "[0e00000000000000000009]",
 "[0.000e00000000000000000009]", "[-0.0e-00000000000000000009]",
 "[1e+00000000000000000009]", "[1.5e-00000000000000000009]",
 "[1e999999999999999999999]", "[1e-999999999999999999999]",
 "[123456789012345678901234567890.5]", "[1e309]", "[1e-400]"].each do |s|
  try("sat #{s}") { JSON.parse(s) }
end

puts "== string strictness (class parity) =="
def tryc(label)
  v = yield
  puts "#{label} => #{v.inspect}"
rescue => e
  puts "#{label} !! #{e.class}"
end
tryc("ctrl-nl")  { JSON.parse("[\"a\nb\"]") }
tryc("ctrl-tab") { JSON.parse("[\"a\tb\"]") }
tryc("ctrl-nul") { JSON.parse("[\"a\u0000b\"]") }
tryc("badhex")   { JSON.parse('["\uZZZZ"]') }
tryc("badhex2")  { JSON.parse('["\u12G4"]') }
tryc("lonehi")   { JSON.parse('["\ud800"]') }
tryc("lonehi2")  { JSON.parse('["\ud800x"]') }
tryc("hipair-badlo") { JSON.parse('["\ud800\ud800"]') }
tryc("badesc")   { JSON.parse('["\x41"]') }
tryc("lonelo-bytes") { JSON.parse('["\udc00"]')[0].bytes }

puts "== sid payloads (verifier item 4: digit runs inside strings) =="
p JSON.parse('{"sid":"1234567890123456789"}')
p JSON.parse('{"s":"a\"1234567890123456789012","n":1}')
p JSON.parse('{"s":"12345678901234567890123","n":12345678901234567890123}')

puts "== error hierarchy (verifier item 7) =="
puts (JSON::NestingError < JSON::ParserError).inspect
puts (JSON::NestingError < JSON::JSONError).inspect
puts (JSON::ParserError < JSON::JSONError).inspect

puts "== cross-parse key sharing =="
k1 = JSON.parse('{"crossparse_key":1}').keys[0]
k2 = JSON.parse('{"crossparse_key":2}').keys[0]
puts "cross equal? #{k1.equal?(k2)} frozen #{k1.frozen?}"

puts "== mixed document round-trip =="
doc = {
  "users" => [
    { "id" => 1, "name" => "Ada", "score" => 99.5, "tags" => ["a", "b"], "on" => true },
    { "id" => 2, "name" => "Bob", "score" => 1e15, "tags" => [], "on" => nil },
  ],
  "total" => 2,
  "ratio" => 2.0 / 3.0,
}
bytes = JSON.generate(doc)
puts bytes
puts (JSON.parse(bytes) == doc).inspect
puts JSON.generate(JSON.parse(bytes)) == bytes ? "stable" : "UNSTABLE"

# ---- 2026-07 exact-number (arbitrary-precision) round ----------------
# The sections below pin the exact-integer/negative-zero/exponent
# surfaces attacked when the whole-document bigint decline was replaced
# by native exact-number handling. VALUE parity is asserted (inspect +
# class + arithmetic re-derivation), not just bytes.

puts "== bigint straddles: value + class parity =="
# Digit-fold instead of Integer(s)/String#to_i: promoting Integer
# arithmetic is exact on both runtimes, while rubyrs's to_i-family
# wraps past i64 (documented core gap, tracked separately) — the
# comparison target must not share the code path under test.
def fold_int(s)
  neg = s.start_with?("-")
  n = (neg ? s[1..] : s).chars.inject(0) { |a, c| a * 10 + (c.ord - 48) }
  neg ? -n : n
end
["9223372036854775806", "9223372036854775807", "9223372036854775808",
 "9223372036854775809", "-9223372036854775807", "-9223372036854775808",
 "-9223372036854775809", "-9223372036854775810",
 "18446744073709551615", "18446744073709551616", "18446744073709551617",
 "12345678901234567890", "-12345678901234567890",
 "123456789012345678901", "-123456789012345678901",
 "999999999999999999", "1000000000000000000"].each do |s|
  v = JSON.parse("[#{s}]")[0]
  puts "#{s} => #{v.inspect} #{v.class} eq=#{(v == fold_int(s)).inspect}"
end
try("30dig-val")  { JSON.parse("[123456789012345678901234567890]")[0] == 123456789012345678901234567890 }
try("100dig-val") { JSON.parse("[" + "9" * 100 + "]")[0] == 10**100 - 1 }
try("neg100dig")  { JSON.parse("[-" + "9" * 100 + "]")[0] == -(10**100 - 1) }
# past f64 range entirely (~1e309): stays exact Integer
try("320dig-val") { JSON.parse("[" + "8" * 320 + "]")[0] == fold_int("8" * 320) }

puts "== bignum in nested containers + keys =="
p JSON.parse('{"a":[{"b":98765432109876543210987}],"c":18446744073709551616}')
p JSON.parse('[[12345678901234567890123456789012345678901234567]]')
puts JSON.generate({ 10**30 => [10**25, -10**25] })
deep = JSON.parse('{"deep":{"deeper":[1,[2,[123456789012345678901234567890]]]}}')
p deep["deep"]["deeper"][1][1][0] == 123456789012345678901234567890
s = "[123456789012345678901234567890,-98765432109876543210]"
puts JSON.generate(JSON.parse(s)) == s ? "big-rt stable" : "big-rt UNSTABLE"

puts "== zero spellings (sign + class) =="
["-0", "0", "-0.0", "0.0", "0e0", "-0e0", "0.0e5", "-0.0e5",
 "0e-5", "-0e-5", "0.000", "-0.000", "-0.0e-5", "-0E5"].each do |s|
  v = JSON.parse("[#{s}]")[0]
  puts "#{s} => #{v.inspect} #{v.class}"
end
try("-0000") { JSON.parse("[-0000]") }
try("-00")   { JSON.parse("[-00]") }
try("00z")   { JSON.parse("[00]") }
try("-0x2")  { JSON.parse('{"z":-0,"f":-0.0}').map { |k, v| [k, v, v.class] } }

puts "== exponent saturation sweep (18/19/20/21 digits x sign x fraction) =="
["1", "1.5", "0.0", "-1", "-1.5", "-0.0", "9.9"].each do |m|
  ["999999999999999999",      # 18 digits, value < i64 max
   "9999999999999999999",     # 19 digits, value > i64 max
   "00000000000000000009",    # 20 digits, value 9
   "099999999999999999999"    # 21 digits
  ].each do |x|
    ["e", "e-", "e+", "E-"].each do |e|
      s = "#{m}#{e}#{x}"
      try("sat #{s}") { JSON.parse("[#{s}]")[0] }
    end
  end
end

puts "== exponent overflow shortcut (adjusted exp > INT32_MAX) =="
# json 2.20 range-checks written_exp - frac_digits BEFORE the mantissa:
# past INT32_MAX the result is mantissa-sign Infinity even for a ZERO
# mantissa ("0.0e2147483649" => Infinity, not 0.0). Probed 2026-07.
["0e2147483647", "0e2147483648", "0e2147483649", "-0e2147483649",
 "0.0e2147483648", "0.0e2147483649", "-0.0e2147483649", "0.0e2147483650",
 "0.00e2147483649", "0.00e2147483650", "0.00000e2147483653",
 "1e2147483649", "-1e2147483649", "1e-2147483649", "1.5e-2147483649",
 "-1.5e-2147483649", "0.0e-2147483649", "0.0e999999999", "0.0e1000000000",
 "0.0e9999999999", "0.0e999999999999999999", "-0.0e999999999999999999",
 "5e2147483646", "5.0e2147483647", "0.0e21474836480"].each do |s|
  try("adj #{s}") { JSON.parse("[#{s}]")[0] }
end

puts "== long-fraction floats (25+ digits, bit fidelity) =="
["3.14159265358979323846264338327950288",
 "0.1234567890123456789012345678901234567890",
 "123456789.123456789012345678901234567",
 "-2.718281828459045235360287471352662497757",
 "0.000000000000000000000000000000001234567890123456789",
 "1.0000000000000000000000000000000000000000000000000001"].each do |s|
  f = JSON.parse("[#{s}]")[0]
  # % (2**64): rubyrs unpack1("Q>") returns the SIGNED reinterpretation
  # for high-bit patterns (core gap, tracked separately); the modulo
  # canonicalizes both runtimes to the unsigned bit pattern.
  puts "#{s} bits=#{([f].pack('G').unpack1('Q>') % (2**64)).to_s(16)}"
end

puts "== overflow boundaries =="
["1e308", "1e309", "-1e309", "1.7976931348623157e308", "1.7976931348623159e308",
 "17976931348623157e292", "-1e-400", "1e-400", "1e999999999999999999",
 "1e-999999999999999999"].each { |s| try("ovb #{s}") { JSON.parse("[#{s}]")[0] } }

puts "== numbers adjacent to every token type =="
p JSON.parse('[123456789012345678901234567890,"s",true,false,null,{"k":98765432109876543210},[1e300],[-0],0.5,18446744073709551616]')
p JSON.parse('{"a":12345678901234567890123,"b":[1,2.5,-0.0],"c":"12345678901234567890123"}')
p JSON.parse("[1,\n 123456789012345678901234567890\t,2]")
p JSON.parse('[123456789012345678901234567890]')

puts "== exact-pairing order (huge floats + bigints interleaved) =="
p JSON.parse('[1e20,123456789012345678901234567890]')
p JSON.parse('[123456789012345678901234567890,1e20]')
p JSON.parse('[-9.3e18,-9223372036854775809]')
p JSON.parse('[-9223372036854775809,-9.3e18]')
v = JSON.parse('[18446744073709551616.0,18446744073709551616]')
puts "#{v.inspect} #{v[0].class}/#{v[1].class}"
v = JSON.parse('[18446744073709551616,18446744073709551616.0]')
puts "#{v.inspect} #{v[0].class}/#{v[1].class}"
v = JSON.parse('[-0.0,-0]')
puts "#{v.inspect} #{v[0].class}/#{v[1].class}"
v = JSON.parse('[-0,-0.0]')
puts "#{v.inspect} #{v[0].class}/#{v[1].class}"
v = JSON.parse('[-0,-1e-400,18446744073709551616,1.8446744073709552e19,-0.0]')
puts "#{v.inspect} #{v.map(&:class).inspect}"
p JSON.parse('{"big":98765432109876543210987654321,"huge":-4.5e19,"z":-0,"s":"-0"}')

puts "== exact-number corner shapes =="
p JSON.parse('{"a":-0,"a":18446744073709551616}')          # dup key, both suspicious
p JSON.parse('{"a":1e20,"b":{"a":-0.0},"a":-0}')           # dup key + nesting
p JSON.parse("[123456789012345678901234567890]   ")         # trailing ws after bigint
deep = "[" * 98 + "123456789012345678901234567890" + "]" * 98
p JSON.parse(deep).flatten(97)[0] == 123456789012345678901234567890
try("deepover") { JSON.parse("[" * 101 + "123456789012345678901234567890" + "]" * 101) && "ok" }

puts "== digit runs in strings stay plain =="
p JSON.parse('{"sid":"123456789012345678901234567890","n":42}')
p JSON.parse('["e12345678901234567890123","1e999 in a string"]')
p JSON.parse('{"e+123456789012345678901":"x","k":"-0"}')
p JSON.parse('["\\"1234567890123456789012345\\""]')
sid_doc = JSON.generate((1..30).map { |i| { "sid" => (1234567890123456789 + i).to_s, "n" => i } })
parsed = JSON.parse(sid_doc)
puts parsed.length
p parsed[0], parsed[29]
puts JSON.generate(parsed) == sid_doc ? "sid stable" : "sid UNSTABLE"
