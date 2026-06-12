# sprintf/format/String#% `%s` must dispatch a user to_s override
# (minitest renders failure reports via "%3d) %s" % [i, result]).
class Foo
  def to_s
    "custom-to-s"
  end
end
f = Foo.new
puts "%s" % [f]
puts "%3d) %s" % [7, f]
puts format("%s!", f)
puts sprintf("[%-12s]", f)
# non-overriding values keep their native rendering
puts "%s %s %s" % [1, :sym, "str"]
# %d on raw ints still works alongside an overriding object
puts "%d %s" % [42, f]
