# The rack-spec advance batch: Exception#set_backtrace/#exception/
# #dup, remove_instance_variable, Enumerable#chain, Marshal probe
# surface, timeout stub. CRuby-identical subset only.
e = RuntimeError.new("boom")
e.set_backtrace(["a.rb:1:in 'x'", "b.rb:2:in 'y'"])
p e.backtrace
e2 = e.exception("wrapped")
p e2.message
p e2.backtrace
p e2.class
p e.exception.equal?(e)
p e.exception(e).equal?(e)
d = e.dup
p [d.message, d.backtrace == e.backtrace, d.equal?(e)]

class IvarBox
  def initialize; @a = 1; @b = 2; end
end
box = IvarBox.new
p box.remove_instance_variable(:@a)
p box.instance_variables
begin
  box.remove_instance_variable(:@zz)
rescue NameError
  puts "NameError: ok"
end

# chain returns CRuby's lazy Enumerator::Chain vs rubyrs's eager
# Array (documented divergence) — compare contents.
p [1, 2].chain([3], [4, 5]).to_a
p (1..2).chain([9]).to_a

require "timeout"
p Timeout.timeout(5) { :ran }
p Timeout::Error < RuntimeError
