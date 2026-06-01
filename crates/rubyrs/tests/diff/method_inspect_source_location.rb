# Method#inspect / UnboundMethod#inspect now embeds the
# ` filename:line` suffix CRuby tacks on. Earlier fixtures
# (method_inspect_format.rb / method_inspect_params.rb /
# method_inspect_singleton.rb) stripped the suffix to dodge
# rubyrs's prior gap; this fixture exercises the now-present
# suffix.
#
# Line numbers themselves are derived from each method's
# proto.op_spans.first().byte_offset — same source as
# Method#source_location. rubyrs's compiler currently stores
# byte 0 for many synthesized protos, so the actual line is
# often 1 rather than the def's real line. We assert
# `inspect` agrees with `source_location` and that the suffix
# is wired through, without pinning specific line numbers.

class C
  def foo
    "C.foo"
  end
  def bar(x, y)
    [x, y]
  end
end

class D < C
  def baz
    "D.baz"
  end
end

m = C.new.method(:foo)
um = C.instance_method(:bar)
inh = D.new.method(:foo)  # inherited from C

# (1) Source-location suffix is present
puts m.inspect.include?(__FILE__)
puts um.inspect.include?(__FILE__)
puts inh.inspect.include?(__FILE__)

# (2) Method#inspect's suffix matches what Method#source_location reports
def suffix_for(m)
  loc = m.source_location
  return "" if loc.nil?
  "#{loc[0]}:#{loc[1]}"
end
puts m.inspect.end_with?("#{suffix_for(m)}>")
puts um.inspect.end_with?("#{suffix_for(um)}>")
puts inh.inspect.end_with?("#{suffix_for(inh)}>")

# (3) Suffix renders for the singleton-method form too
obj = C.new
def obj.sing; "sing"; end
sm = obj.method(:sing)
puts sm.inspect.include?(__FILE__)
puts sm.inspect.end_with?("#{suffix_for(sm)}>")

# (4) Format prefix still works (regression — earlier
# fixtures already exercise this but verifying here
# alongside the suffix).
puts m.inspect.start_with?("#<Method: C#foo()")
puts um.inspect.start_with?("#<UnboundMethod: C#bar(x, y)")
puts inh.inspect.start_with?("#<Method: D(C)#foo()")
puts sm.inspect.start_with?("#<Method: #<C:0x")
