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
