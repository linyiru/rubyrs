# Integer predicates
puts 4.even?
puts 4.odd?
puts 5.even?
puts 5.odd?
puts 0.zero?
puts 1.zero?
puts 5.positive?
puts (-5).positive?
puts 0.positive?
puts (-5).negative?
puts 5.negative?
puts 0.negative?

# Integer absolute / succ / pred / to_i
puts (-7).abs
puts 7.abs
puts 0.abs
puts 5.succ
puts 5.next
puts 5.pred
puts 42.to_i

# Integer#upto
sum = 0
1.upto(5) { |i| sum = sum + i }
puts sum

# Integer#downto
out = []
3.downto(1) { |i| out << i }
puts out[0]
puts out[1]
puts out[2]

# upto with start > stop runs zero times
n = 0
5.upto(1) { |_i| n = n + 1 }
puts n

# downto with start < stop runs zero times
n2 = 0
1.downto(5) { |_i| n2 = n2 + 1 }
puts n2

# --- String methods ---
puts "hello".length
puts "hello".size
puts "".empty?
puts "x".empty?
puts "HELLO".downcase
puts "hello".upcase
puts "Hello".reverse
puts "   spaced   ".strip
puts "   left".lstrip
puts "right   ".rstrip
puts "hello world".include?("world")
puts "hello world".include?("nope")
puts "hello".start_with?("he")
puts "hello".start_with?("lo")
puts "hello".end_with?("lo")
puts "hello".end_with?("he")
puts "ab" * 3
puts "abc" * 0

# Integer#to_s + String#length/size chaining keeps Ruby-visible results.
puts 0.to_s.length
puts 9.to_s.length
puts 10.to_s.size
puts (-10).to_s.length

# Non-Integer receivers must still dispatch through user Ruby code.
class WeirdToSLength
  def to_s
    [1, 2, 3]
  end
end
puts WeirdToSLength.new.to_s.length

# Lex comparisons
puts "apple" < "banana"
puts "banana" > "apple"
puts "apple" == "apple"
puts "apple" != "banana"
puts "apple" != "apple"

# String#to_i
puts "42".to_i
puts "-13".to_i
puts "  123abc".to_i
puts "abc".to_i
puts "".to_i
puts "+99".to_i

# String#chars
chars = "abc".chars
puts chars.length
puts chars[0]
puts chars[1]
puts chars[2]

# String#split — default (whitespace)
parts = "  hello   world  foo".split
puts parts.length
puts parts[0]
puts parts[1]
puts parts[2]

# String#split — explicit separator
csv = "a,b,c,d".split(",")
puts csv.length
puts csv[0]
puts csv[3]

# String#split — empty separator yields chars
each_char = "xyz".split("")
puts each_char.length
puts each_char[0]
puts each_char[2]

# String#to_sym round-trip
sym = "hello".to_sym
puts sym
puts sym == :hello

# Chaining: split then map then count
words = "the quick brown fox".split.map { |w| w.upcase }
puts words[0]
puts words[3]
puts words.length

# Realistic-ish usage inside a class
class Greeter
  def initialize(name)
    @name = name
  end
  def shout
    "HELLO, #{@name.upcase}!"
  end
end
puts Greeter.new("world").shout
