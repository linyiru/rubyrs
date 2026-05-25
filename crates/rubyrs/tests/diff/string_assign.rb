# String#[]= — in-place character / substring replacement.
# Works because Value::Str now holds Rc<RefCell<String>> so all
# aliases see the mutation.

# Single index — replace one character.
s = "hello"
s[0] = "H"
puts s
s[4] = "O"
puts s
s[-1] = "!"
puts s

# Two-arg slice — replace N characters from index I.
s = "abcdef"
s[1, 2] = "XYZ"
puts s         # "aXYZdef"

s = "abcdef"
s[1, 0] = "!"     # insert at position 1
puts s

s = "abcdef"
s[2, 4] = ""      # delete from index 2 onward
puts s

s = "abcdef"
s[0, 6] = "REPLACED"
puts s

# Replacement strings of different sizes.
s = "abcdef"
s[2, 1] = "XX"     # grow
puts s

s = "abcdef"
s[1, 4] = "_"      # shrink
puts s

# Negative index in single-arg.
s = "abcde"
s[-1] = "Z"
puts s
s[-5] = "A"
puts s

# Aliasing: a second reference sees the change because of shared
# RefCell — though Ruby's String mutation is the primary path,
# the aliasing semantics demonstrate the refcell shape works.
a = "hello"
b = a
a[0] = "J"
puts a
puts b   # CRuby: also "Jello" — `b = a` shares the same object

# Mutation in a method.
def yell!(s)
  s[0] = s[0].upcase
end

word = "hello"
yell!(word)
puts word

# IndexError when out of range.
begin
  bad = "abc"
  bad[100] = "x"
rescue IndexError => e
  puts "rescued: #{e.message}"
end

begin
  bad2 = "abc"
  bad2[-100] = "y"
rescue IndexError => e
  puts "rescued: #{e.message}"
end

# Inside a builder loop.
buf = "    "
"WXYZ".chars.each_with_index do |c, i|
  buf[i] = c
end
puts buf

# Class wrapping mutable string.
class Greeter
  def initialize(name)
    @name = name
  end
  def shout!
    @name[0] = @name[0].upcase
  end
  def name
    @name
  end
end

g = Greeter.new("alice")
g.shout!
puts g.name
