# A brace/do block passed alongside a splat argument must still reach the callee.
def m(*a, &b)
  puts "given=#{block_given?} args=#{a.inspect}"
  b.call(a.sum) if b
end

m(*[1, 2]) { |n| puts "brace #{n}" }
m(*[3, 4]) do |n| puts "do #{n}" end
m(5) { |n| puts "noblock-splat #{n}" }

# Struct.new with a splat key list AND a body block (regexp_parser's Token pattern).
KEYS = [:type, :text]
Token = Struct.new(*KEYS) do
  attr_accessor :previous, :nxt
end
t = Token.new(:a, "b")
t.nxt = 5
puts t.nxt
puts t.type
puts t.text
