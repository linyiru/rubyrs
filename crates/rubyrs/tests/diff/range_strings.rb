# Range over String endpoints — `('a'..'z').each` and friends, driven
# by String#succ. CRuby supports Range over any object that responds to
# `succ` / `<=>`; rubyrs's subset focuses on the canonical String case.

# Block iteration
out = []
("a".."e").each { |c| out << c }
puts out.inspect

# Exclusive range stops one short.
exc = []
("a"..."d").each { |c| exc << c }
puts exc.inspect

# to_a
puts ("a".."c").to_a.inspect
puts ("p".."t").to_a.inspect

# count
puts ("a".."e").count
# CRuby's Range#size on String endpoints returns nil
# (it's defined only for numeric ranges). rubyrs matches —
# we expose count instead for the "how many?" use case.
p ("a".."e").size              # nil

# include? / cover?
puts ("a".."f").include?("c")    # true
puts ("a".."f").include?("g")    # false
puts ("a"..."f").include?("f")   # false (exclusive)
puts ("a".."f").cover?("d")      # true

# String#succ standalone
puts "a".succ                    # b
puts "y".succ                    # z
puts "z".succ                    # aa
puts "Az".succ                   # Ba
puts "1".succ                    # 2
puts "9".succ                    # 10

# Map over an alphabetic range using a `for c in range` style
# (well, with each + map).
caps = ("a".."e").to_a.map { |c| c.upcase }
puts caps.inspect

# Use inside a class — DSL idiom that mixes literal-range with iteration.
class Alphabet
  def vowels_in(r)
    out = []
    r.each { |c| out << c if "aeiou".include?(c) }
    out
  end
end
puts Alphabet.new.vowels_in("a".."m").inspect
