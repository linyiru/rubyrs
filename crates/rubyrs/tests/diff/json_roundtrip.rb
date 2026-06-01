# Pure-Ruby JSON (ADR 0026 menu item 2) parity gate. Exercises the public
# surface against CRuby's `json` gem as the oracle. Runs under
# `--features stdlib` only (registered #[cfg(feature = "stdlib")]).
#
# Floats are kept to everyday values whose Float#to_s matches CRuby — the
# scientific-notation edge cases are a documented Tier-1 divergence
# (SUBSET.md / ADR 0019 class `h`) and are intentionally excluded.

require 'json'

# --- parse: scalars, nesting, all value types ---
puts JSON.parse('{"a":1,"b":[2,3],"c":{"d":true,"e":null}}').inspect
puts JSON.parse('[1,2.5,"x",true,false,null]').inspect
puts JSON.parse('"just a string"').inspect
puts JSON.parse('42').inspect
puts JSON.parse('-17').inspect
puts JSON.parse('3.5').inspect
puts JSON.parse('true').inspect

# --- parse: string escapes + unicode (\uXXXX -> the chr primitive) ---
puts JSON.parse('"tab\tnewline\nquote\"slash\\\\"').inspect
puts JSON.parse('"é € 中"').inspect
puts JSON.parse('"😀"').inspect          # surrogate pair -> emoji

# --- parse: symbolize_names option ---
puts JSON.parse('{"name":"ada","tags":["x","y"]}', symbolize_names: true).inspect

# --- generate: round-trip and explicit values ---
puts JSON.generate({"a" => 1, "b" => [2, 3], "c" => {"d" => true, "e" => nil}})
puts JSON.generate([1, 2.5, "x", true, false, nil])
puts JSON.generate("needs \"escaping\"\n\ttab")
puts JSON.generate({"int" => 0, "neg" => -5, "f" => 1.5})

# --- non-ASCII passes through (CRuby JSON does not \u-escape by default) ---
puts JSON.generate({"greet" => "héllo €", "cjk" => "中文"})

# --- to_json mixin on core types ---
puts({"k" => [1, "two", nil]}.to_json)
puts [1, 2, 3].to_json
puts "str".to_json
puts 42.to_json
puts nil.to_json
puts true.to_json

# --- pretty_generate ---
puts JSON.pretty_generate({"a" => 1, "nested" => {"b" => [1, 2]}, "arr" => []})

# --- round-trip stability ---
doc = {"users" => [{"id" => 1, "name" => "A"}, {"id" => 2, "name" => "B"}], "ok" => true}
puts JSON.parse(JSON.generate(doc)) == doc

# --- parse errors raise JSON::ParserError ---
["{", "[1,]", "{'bad':1}", "nope", ""].each do |bad|
  begin
    JSON.parse(bad)
    puts "no-error: #{bad.inspect}"
  rescue JSON::ParserError
    puts "ParserError: #{bad.inspect}"
  end
end
