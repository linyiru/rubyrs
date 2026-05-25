# Regex form of String#sub / #gsub, including the block form and
# backref substitution. Adds to the existing literal-pattern forms.

s = "hello world"

# Basic regex sub/gsub.
puts s.sub(/o/, "0")             # hell0 world (first only)
puts s.gsub(/o/, "0")            # hell0 w0rld
puts s.gsub(/[aeiou]/, "*")      # h*ll* w*rld

# Block form.
puts s.gsub(/(\w+)/) { |m| m.upcase }     # HELLO WORLD
puts "a1 b2 c3".gsub(/\d/) { |d| "<#{d}>" }  # a<1> b<2> c<3>

# Block-return is coerced to string (non-String values pass
# through Value::to_display, so e.g. Integer becomes its decimal
# string form).
puts "a1 b2".gsub(/\d/) { |d| d.to_i * 10 }   # a10 b20

# break inside the block returns the break value, not the
# partially-built string.
result = s.gsub(/\w+/) { |w| break "stopped" if w == "world"; w.upcase }
puts result                       # stopped

# Backref substitution in the replacement template.
puts s.sub(/(\w+) (\w+)/, '\2 \1')   # world hello
puts "abc".gsub(/./, '<\0>')         # <a><b><c>
puts "abc".gsub(/(.)(.)/, '\2\1')    # ba c

# Mixed with the existing literal-pattern forms (regression check).
puts s.sub("hello", "HI")            # HI world
puts s.gsub("l", "L")                # heLLo worLd

# Inside a method.
class Slugify
  def call(s)
    s.downcase.gsub(/\s+/, "-").gsub(/[^a-z0-9-]/, "")
  end
end
puts Slugify.new.call("Hello, World!  ")     # hello-world- (trailing dash from trailing spaces)
