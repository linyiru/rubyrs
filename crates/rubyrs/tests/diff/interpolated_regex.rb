# `/pre #{x} post/` — InterpolatedRegularExpressionNode. Built
# pattern compiles at the point of evaluation, then participates
# in every regex entry point the same way a literal `/.../`
# would: match?, =~, $1..$N, $~.

prefix = "abc"
suffix = "xyz"
re = /\A#{prefix}\d+#{suffix}\z/
puts "hit: #{re.match?("abc123xyz")}"
puts "miss: #{re.match?("xxx")}"

# Captures from an interpolated pattern populate $1/$2
"hello world" =~ /(\w+)\s+#{"world"}/
puts "capture: $1=#{$1}"

# Multi-part interpolation
a = "foo"
b = "bar"
re2 = /^#{a}-#{b}$/
puts "multi: #{re2.match?("foo-bar")}"

# Empty interpolation slot — equivalent to the surrounding pattern
empty = ""
re3 = /^#{empty}done$/
puts "empty: #{re3.match?("done")}"

# Pattern reuse via cache — repeated identical expansion
2.times do
  puts(/#{"abc"}/ =~ "xabcy")
end
