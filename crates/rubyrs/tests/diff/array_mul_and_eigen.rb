# Array#* — Int repetition + String join alias.
p [1, 2] * 3
p ["aa", "ab"] * 2
p [] * 5
p [1, [2]] * 2
p [1, 2, 3] * ","
begin
  [1] * -1
rescue ArgumentError => e
  puts "neg: #{e.message}"
end
# Object#singleton_class — instance eigenclass materialization;
# `class << self; self; end` inside a method desugars to it
# (minitest's Object#stub metaclass idiom).
class Foo; end
f = Foo.new
def f.probe
  class << self; self; end
end
p f.probe.class
p f.probe.equal?(f.probe)
class Bar
  def meta
    class << self; self; end
  end
end
p Bar.new.meta.class
o = Object.new
o.singleton_class.send(:define_method, :hi) { "hi-eigen" }
p o.hi
# Proc#call with a block-pass argument dispatches (the callee here
# ignores the incoming block — the dominant gem shape).
f2 = ->(*args) { args.sum }
blk = proc { 99 }
p f2.call(1, 2, &blk)
p f2.(3, &blk)
