# Object#dup and #clone invoke the `initialize_copy` hook after the
# shallow ivar copy, so a class can deep-copy mutable ivars. CRuby calls
# it via initialize_dup / initialize_clone. rack's Rack::Request defines
# `initialize_copy` to dup @env so `req.dup.env` is a distinct hash.

class Box
  attr_accessor :items, :tag
  def initialize_copy(other)
    @items = other.items.dup     # deep-copy the mutable array
  end
end

a = Box.new
a.items = [1, 2, 3]
a.tag = "orig"

b = a.dup
p b.items == a.items           # true (same contents)
p b.items.equal?(a.items)      # false (initialize_copy duped it)
b.items << 4
p a.items                      # [1, 2, 3] (original unaffected)

c = a.clone
p c.items.equal?(a.items)      # false (clone also runs initialize_copy)

# a class WITHOUT initialize_copy keeps the default shallow-share
class Plain
  attr_accessor :data
end
p1 = Plain.new
p1.data = [9]
p2 = p1.dup
p p2.data.equal?(p1.data)      # true (shallow; no custom hook)

# initialize_copy receives the original
class Spy
  attr_reader :seen
  def initialize_copy(other)
    @seen = other.object_id
  end
end
s = Spy.new
p s.dup.seen == s.object_id    # true
