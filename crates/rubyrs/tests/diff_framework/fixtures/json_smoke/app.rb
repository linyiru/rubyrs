# Tier-1 JSON parity smoke — same app.rb runs on rubyrs (via
# flori-json-cext parser.bundle) and CRuby (via stdlib json/ext).
# Both expose `JSON::Ext::Parser`; we exercise five primitive
# flavours (int, float, string, bool, null) plus an array, and
# emit one labelled line per element so a regression points at
# the exact diverging element.
require_relative "compat"

JSON_INPUT = '{"name":"rubyrs","ver":4.7,"tags":["ruby","rust"],"ok":true,"nope":false,"nil":null,"count":42}'

result = JSON::Ext::Parser.new(JSON_INPUT).parse

puts "class=#{result.class}"
puts "name=#{result["name"]}"
puts "name.class=#{result["name"].class}"
puts "ver=#{result["ver"]}"
puts "ver.class=#{result["ver"].class}"
puts "tags=#{result["tags"].inspect}"
puts "tags.class=#{result["tags"].class}"
puts "ok=#{result["ok"]}"
puts "nope=#{result["nope"]}"
puts "nil_is_nil=#{result["nil"].nil?}"
puts "count=#{result["count"]}"
puts "count.class=#{result["count"].class}"
