# enum_for / to_enum must capture keyword args SEPARATELY from
# positional args and replay them as keywords when the Enumerator is
# driven (CRuby flags Kernel#enum_for ruby2_keywords). Driver: the
# pure-Ruby Find shim's `enum_for(:find, *paths, ignore_error: true)`,
# whose `find` re-invocation otherwise saw the options Hash as an extra
# positional path. Also exercises the >3-arg replay (old code capped at
# 3 positional args).
module M
  def walk(*paths, ignore_error: true, depth: 0)
    return enum_for(:walk, *paths, ignore_error: ignore_error, depth: depth) unless block_given?
    paths.each { |p| yield [p, ignore_error, depth] }
  end
  module_function :walk
end

p M.walk("a", "b").to_a
p M.walk("a", "b", ignore_error: false, depth: 3).to_a
p M.walk("a", "b", "c", "d", "e").to_a   # >3 positional, no kwargs
p M.walk.to_a                            # no args at all

# Positional Hash stays positional (not absorbed as kwargs)
class C
  include Enumerable
  def each_pair(*rows)
    return enum_for(:each_pair, *rows) unless block_given?
    rows.each { |r| yield r }
  end
end
p C.new.each_pair({a: 1}, {b: 2}).to_a
