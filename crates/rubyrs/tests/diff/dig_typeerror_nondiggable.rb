# Hash#dig / Array#dig raise TypeError when an intermediate value is not
# nil and does not support #dig but more keys remain — matching CRuby
# ("String does not have #dig method"). A nil intermediate short-circuits
# to nil; a value that defines its own #dig is dispatched to.
# (rack's Rack::Headers#test_dig: `@fh.dig('AB', 1)` raises TypeError.)

h = { "a" => "1", "b" => [10, 20], "c" => { "d" => 5 } }

# diggable intermediates keep working
p h.dig("b", 1)            # 20
p h.dig("c", "d")          # 5
p h.dig("a")               # "1"

# nil short-circuits (no error even with trailing keys)
p h.dig("missing", 0, 1)   # nil

# non-diggable intermediate + remaining key => TypeError
begin
  h.dig("a", 0)
rescue TypeError => e
  puts "TypeError: #{e.message}"   # String does not have #dig method
end

begin
  { "x" => 5 }.dig("x", 0)
rescue TypeError => e
  puts "TypeError: #{e.message}"   # Integer does not have #dig method
end

# Array#dig nested
p [1, [2, [3, 4]]].dig(1, 1, 0)    # 3

# a custom object defining #dig is dispatched to
class Digger
  def dig(k) = "dug:#{k}"
end
p({ "o" => Digger.new }.dig("o", :z))   # "dug:z"
