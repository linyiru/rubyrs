# AS-lite spike — exercises the ActiveSupport core-ext surface
# every Rack-shape Ruby web app reaches for. The goal is NOT to
# implement anything yet; it's to produce a complete gap report
# by running this script under both runtimes and diffing the
# `[OK]` / `[GAP]` lines.
#
# Run:
#   ruby                poc/as_lite/spike.rb              # CRuby + AS
#   target/release/rubyrs poc/as_lite/spike.rb            # rubyrs stock
#
# The `probe` helper wraps each idiom in begin/rescue so the
# script runs to completion on rubyrs even when every line raises
# NoMethodError; that's what makes the gap inventory complete
# rather than truncated at the first miss.
#
# Output shape per line:
#   [OK]   <label>: <inspect of result>
#   [GAP]  <label>: <ExceptionClass>: <message snippet>
# `[GAP]` lines on rubyrs (paired with `[OK]` on CRuby) are the
# method names that the eventual `src/stdlib_vendor/active_support_lite.rb`
# canon needs to implement.
require_relative "compat"

puts "runtime: #{RUNTIME_LABEL}"
puts ""

def probe(label, &block)
  result = block.call
  puts "[OK]   #{label}: #{result.inspect}"
rescue => e
  msg = e.message.to_s
  msg = msg[0..70] if msg.length > 70
  puts "[GAP]  #{label}: #{e.class}: #{msg}"
end

puts "--- blank? / present? / presence ---"
probe("nil.blank?")              { nil.blank? }
probe("''.blank?")               { "".blank? }
probe("'   '.blank?")            { "   ".blank? }
probe("'x'.blank?")              { "x".blank? }
probe("[].blank?")               { [].blank? }
probe("{}.blank?")               { {}.blank? }
probe("0.blank?")                { 0.blank? }
probe("false.blank?")            { false.blank? }
probe("true.blank?")             { true.blank? }
probe("'  '.present?")           { "  ".present? }
probe("'x'.present?")            { "x".present? }
probe("nil.presence")            { nil.presence }
probe("''.presence")             { "".presence }
probe("'x'.presence")            { "x".presence }

puts ""
puts "--- String ---"
probe("'  hello world  '.squish")    { "  hello   world  ".squish }
probe("'active_record'.camelize")    { "active_record".camelize }
probe("'active_record'.camelize(:lower)") { "active_record".camelize(:lower) }
probe("'ActiveRecord'.underscore")   { "ActiveRecord".underscore }
probe("'puma_server'.dasherize")     { "puma_server".dasherize }
probe("'puni puni'.titleize")        { "puni puni".titleize }
probe("'employee_id'.humanize")      { "employee_id".humanize }
probe("'Once upon a time'.truncate(15)") { "Once upon a time".truncate(15) }

puts ""
puts "--- Hash ---"
probe("Hash#slice")             { {a: 1, b: 2, c: 3}.slice(:a, :c) }
probe("Hash#except")            { {a: 1, b: 2, c: 3}.except(:b) }
probe("Hash#transform_values")  { {a: 1, b: 2}.transform_values { |v| v * 10 } }
probe("Hash#transform_keys")    { {a: 1, b: 2}.transform_keys(&:to_s) }
probe("Hash#symbolize_keys")    { {"a" => 1, "b" => 2}.symbolize_keys }
probe("Hash#stringify_keys")    { {a: 1, b: 2}.stringify_keys }
probe("Hash#deep_stringify")    { {a: {b: {c: 1}}}.deep_stringify_keys }
probe("Hash#deep_symbolize")    { {"a" => {"b" => 1}}.deep_symbolize_keys }
probe("Hash#deep_merge")        { {a: 1, b: {c: 2}}.deep_merge({a: 9, b: {d: 3}}) }
probe("Hash#compact")           { {a: 1, b: nil, c: 3}.compact }

puts ""
puts "--- Array ---"
probe("[1,2,3].second")         { [1,2,3].second }
probe("[1,2,3,4].third")        { [1,2,3,4].third }
probe("[1,2,3,4,5].fourth")     { [1,2,3,4,5].fourth }
probe("Array#in_groups_of(3)")  { [1,2,3,4,5,6,7].in_groups_of(3) }
probe("Array#in_groups_of(3,false)") { [1,2,3,4,5,6,7].in_groups_of(3, false) }
probe("[].blank?")              { [].blank? }
probe("Array#to (CRuby Array#first(n))") { [1,2,3,4,5].first(3) }

puts ""
puts "--- Numeric ---"
probe("0.blank?")               { 0.blank? }
probe("42.present?")             { 42.present? }
probe("3.minutes")              { 3.minutes }
probe("1.day")                  { 1.day }
probe("2.hours.ago")            { 2.hours.ago }

puts ""
puts "--- Range / Object ---"
probe("(1..5).to_a")            { (1..5).to_a }
probe("Object#try (nil)")       { nil.try(:upcase) }
probe("Object#try (existing)")  { "hi".try(:upcase) }
probe("Object#try (missing)")   { "hi".try(:not_a_method) }
probe("Object#in?")             { 2.in?([1,2,3]) }

puts ""
puts "--- Time / Date (likely deferred but useful to probe) ---"
probe("Time.current")           { Time.current }
probe("Time.zone")              { Time.zone }
probe("1.day.from_now.class")   { 1.day.from_now.class }

puts ""
puts "spike complete."
