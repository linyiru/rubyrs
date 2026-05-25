# String mutation family — `<<`, `concat`, `prepend`, `replace`.
# All mutate the receiver in place; aliases of the same String
# see the change (Value::Str = Rc<RefCell<String>>).

# << — append one item. CRuby accepts String or Integer (codepoint).
s = "hello"
s << " "
s << "world"
p s

# << with Integer codepoint.
b = "abc"
b << 33   # '!'
p b
b << 0x41 # 'A'
p b

# concat — variadic append.
m = "msg:"
m.concat(" ", "ready", " ", "to", " ", "go")
p m

# Returns the receiver.
r = "x"
res = r << "y"
p r
p res
p r.equal?(res)

# prepend — prepend to start. Multiple args concatenate in order
# THEN prepend (prefix = args.join + receiver).
p1 = "world"
p1.prepend("hello, ")
p p1

p2 = "end"
p2.prepend("a-", "b-", "c-")
p p2

# replace — clobber the entire content.
rep = "hello"
rep.replace("goodbye")
p rep

# Aliasing: mutation through one variable is visible to others.
a = "starter"
b = a
a << " kit"
p a
p b

# Inside an iterator: build a string from elements.
buf = ""
[1, 2, 3].each { |n| buf << "#{n}-" }
p buf

# Idiomatic StringBuffer pattern.
class TextBuilder
  def initialize
    @buf = ""
  end
  def add(s)
    @buf << s
    self
  end
  def add_line(s)
    @buf << s << "\n"
    self
  end
  def result
    @buf
  end
end

t = TextBuilder.new
t.add("hello").add(", ").add("world").add_line("!")
t.add_line("second line")
puts t.result

# concat returns self even with zero args.
z = "noop"
z.concat
p z

# Empty receiver still works.
e = ""
e << "a" << "b" << "c"
p e

# Chain with concat after prepend.
c = "core"
c.prepend("[pre-]")
c.concat("[-post]")
p c
