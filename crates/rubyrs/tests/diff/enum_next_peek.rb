# Enumerator#next / peek / rewind via eager materialization. CRuby drives
# the source lazily through a Fiber; rubyrs materializes the whole
# enumeration on first use and walks it with a cursor. Finite enumerators
# (incl. StopIteration at the end, rescued by `loop`) behave identically.
e = [1, 2].each
p e.next                                  # 1
p e.next                                  # 2
begin; e.next; rescue StopIteration => ex; p ex.message; end  # "iteration reached an end"

# peek does not advance; rewind restarts
e2 = [10, 20].each
p e2.peek                                 # 10
p e2.next                                 # 10
p e2.peek                                 # 20
e2.rewind
p e2.next                                 # 10

# peek on an empty enumerator raises StopIteration
begin; [].each.peek; rescue StopIteration; p :peek_empty_raised; end

# the canonical loop + next idiom (loop rescues StopIteration)
e3 = [:a, :b, :c].each
out = []
loop { out << e3.next }
p out                                     # [:a, :b, :c]

# next over a multi-value source (Hash#each yields [k, v])
he = {a: 1, b: 2}.each
p he.next                                 # [:a, 1]
p he.next                                 # [:b, 2]

# next over an enum_for-built enumerator
class NextC
  def go
    return enum_for(:go) unless block_given?
    yield 7; yield 8; yield 9
  end
end
g = NextC.new.go
p g.next                                  # 7
p g.peek                                  # 8
p g.next                                  # 8

# next over a generator-form enumerator
gen = Enumerator.new { |y| y << :x; y << :y }
p gen.next                                # :x
p gen.next                                # :y

# external iteration and to_a are independent
e4 = [100, 200, 300].each
p e4.next                                 # 100
p e4.to_a                                 # [100, 200, 300]
