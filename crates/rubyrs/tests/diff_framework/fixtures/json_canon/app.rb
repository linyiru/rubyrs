# Tier-1 JSON canon parity — runs `require "json"` on both
# runtimes. CRuby loads stdlib json; rubyrs resolves to the
# embedded pure-Ruby canon at src/stdlib_vendor/json.rb (gated
# behind the `stdlib` Cargo feature — framework-parity CI job
# builds with `--features default,stdlib,...`). No engine-aware
# compat shim — the require name is identical, the API surface
# the script touches is the cross-impl-stable subset.
require "json"

# ---- generate over canonical types ----
puts JSON.generate(nil)
puts JSON.generate(true)
puts JSON.generate(false)
puts JSON.generate(0)
puts JSON.generate(42)
puts JSON.generate(-17)
puts JSON.generate("")
puts JSON.generate("hello")
puts JSON.generate("a\"b\\c\nd\te")
puts JSON.generate([])
puts JSON.generate([1, 2, 3])
puts JSON.generate(["a", true, nil, 42])
puts JSON.generate({})
puts JSON.generate({"k" => "v"})
puts JSON.generate({"name" => "rubyrs", "ver" => 4, "ok" => true, "tags" => ["ruby", "rust"]})

# Symbol-key stringification (CRuby's `to_s`-on-key convention).
puts JSON.generate({a: 1, b: [2, 3]})

# Nested structures.
nested = {
  "level1" => {
    "level2" => {
      "level3" => ["deep", true, nil, 0]
    }
  }
}
puts JSON.generate(nested)

# Control-char escaping ranges (< 0x20 → \uXXXX).
puts JSON.generate("\x00\x01\x1F end")

# ---- parse + round-trip ----
SAMPLES = [
  'null',
  'true',
  'false',
  '42',
  '-7',
  '0',
  '"hello"',
  '"with \"quotes\" and \\\\"',
  '"newline \n tab \t"',
  '[]',
  '[1, 2, 3]',
  '["a", true, null, 0]',
  '{}',
  '{"k": "v"}',
  '{"name":"rubyrs","ver":4,"tags":["ruby","rust"],"ok":true,"nope":false,"nil":null,"count":42}',
  '  [  1  ,  2  ,  3  ]  ',
  '{"nested":{"deep":{"value":[1,true,null]}}}'
]

SAMPLES.each do |src|
  parsed = JSON.parse(src)
  # Print the inspect of the parsed value AND the re-serialised
  # form — covers both "did we parse the right shape?" and
  # "did we serialise it back to the canonical compact form?"
  puts "parse: #{parsed.inspect}"
  puts "gen:   #{JSON.generate(parsed)}"
end

# ---- error paths ----
# We compare ONLY the exception class, not the message text —
# error messages aren't part of the API contract and CRuby's
# stdlib + the pure-Ruby canon legitimately differ on phrasing.
begin
  JSON.parse("{bad")
rescue JSON::ParserError => e
  puts "err.class=#{e.class}"
end

begin
  JSON.parse("[1, 2,")
rescue JSON::ParserError => e
  puts "err.class=#{e.class}"
end

# ---- pretty_generate ----
puts "--- pretty_generate ---"
puts JSON.pretty_generate([])
puts JSON.pretty_generate({})
puts JSON.pretty_generate(42)
puts JSON.pretty_generate("hi")
puts JSON.pretty_generate([1, 2, 3])
puts JSON.pretty_generate({"a" => 1, "b" => "x"})
puts JSON.pretty_generate({"a" => [1, 2], "b" => {"c" => true}})

# ---- to_json mixin ----
puts "--- to_json ---"
puts 42.to_json
puts "hi".to_json
puts nil.to_json
puts true.to_json
puts false.to_json
puts [1, 2].to_json
puts({"a" => 1}.to_json)
puts :sym.to_json

# ---- dump / load ----
puts "--- dump / load ---"
puts JSON.dump({"a" => 1})
puts JSON.load('{"a":1}').inspect

# ---- symbolize_names ----
# Pass via positional Hash (NOT kwargs-shortcut syntax) — the
# embedded canon's parse signature is `parse(str, opts = nil)`
# so a literal Hash is the cross-runtime compatible call style.
puts "--- symbolize_names ---"
puts JSON.parse('{"a":1,"b":{"c":2}}', { symbolize_names: true }).inspect

# ---- JSON::State ----
# State is the formatting-options bag CRuby uses internally;
# we expose it as forward-compat for callers that build a
# reusable State and pass it to generate(). Cross-runtime
# parity over the accessors + the generate(obj, state) path.
puts "--- JSON::State ---"
s = JSON::State.new(indent: "    ", space: "  ", allow_nan: true)
puts s.indent.inspect
puts s.space.inspect
puts s.allow_nan?
puts JSON.generate({"a" => 1}, s)

# ---- max_nesting ----
# Default-deep payload (>100 levels) rejected; explicit
# max_nesting opt enforced; class is JSON::NestingError, base
# is JSON::JSONError. Tests both parse and generate paths.
puts "--- max_nesting ---"
begin
  JSON.parse("[" * 200 + "]" * 200)
  puts "default-deep parse: no error"
rescue JSON::NestingError => e
  puts "default-deep parse: NestingError"
end
begin
  JSON.generate([[[[1]]]], { max_nesting: 2 })
  puts "deep gen: no error"
rescue JSON::NestingError => e
  puts "deep gen: NestingError"
end
begin
  JSON.parse("[[[[1]]]]", { max_nesting: 2 })
  puts "deep parse: no error"
rescue JSON::NestingError => e
  puts "deep parse: NestingError"
end

# ---- Exception base class ----
# `JSON::JSONError` is the shared parent of Parser / Nesting /
# Generator errors — user code can rescue the base class instead
# of enumerating each subclass. Generator path tested via the
# allow_nan=false Float-NaN reject (a fall-through to `to_s` is
# CRuby's Object-arg behaviour, which is class-`h` divergence
# from our stricter raise; the fixture stays on the bit-exact
# subset).
puts "--- JSONError base ---"
begin
  JSON.parse("xxx")
rescue JSON::JSONError => e
  puts "JSONError caught: #{e.class}"
end
begin
  JSON.generate(0.0 / 0.0)
rescue JSON::JSONError => e
  puts "JSONError caught: #{e.class}"
end

# ---- Object#to_json fall-through ----
# Vanilla CRuby `json` defines Object#to_json that wraps to_s.
# Our canon mirrors that for cross-runtime parity. Use a class
# with a deterministic to_s so the byte-diff is stable.
puts "--- Object#to_json fall-through ---"
class JCanonProbe
  def initialize(x); @x = x; end
  def to_s; "JCanonProbe(@x=#{@x})"; end
end
puts JCanonProbe.new(7).to_json

# ---- JSON[] shortcut ----
puts "--- JSON[] ---"
puts JSON['{"a":1,"b":2}']
puts JSON[{"a" => 1}]
puts JSON[[1, 2, 3]]

# ---- JSON.unparse / pretty_unparse aliases ----
puts "--- unparse aliases ---"
puts JSON.unparse({"a" => 1})
puts JSON.pretty_unparse({"a" => [1, 2]})

# ---- to_json(state) on Array / Hash ----
puts "--- to_json(state) ---"
state = JSON::State.new(indent: "    ", space: " ", object_nl: "\n", array_nl: "\n")
puts [1, 2, 3].to_json(state)
puts({"a" => [1, 2]}.to_json(state))

# ---- parse! accepts NaN / Infinity ----
puts "--- parse! NaN/Infinity ---"
puts JSON.parse!('NaN').nan?
puts JSON.parse!('Infinity').infinite?
puts JSON.parse!('-Infinity').infinite?
# parse! also accepts top-level scalars (same as parse here —
# the rubyrs Parser doesn't enforce JSON's "value must be an
# object or array" historical restriction even in plain parse).
puts JSON.parse!('42')
puts JSON.parse!('"hello"')
