# Object.const_set installs at the TOP LEVEL under the bare name —
# Object's constants ARE the toplevel constants. minitest's const
# round-trip tests remove and restore RuntimeError this way; the
# restored class must serve `raise "msg"` again.
RT = RuntimeError
Object.send :remove_const, :RuntimeError
Object.const_set :RuntimeError, RT
begin
  raise "boom"
rescue => e
  p [e.class, e.message]
end
# value constants round-trip too
Object.const_set :SOME_TOP, 42
p SOME_TOP
p Object.const_get(:SOME_TOP)
Object.send :remove_const, :SOME_TOP
p defined?(SOME_TOP)
# anonymous class installed at toplevel gains the name
k = Class.new(StandardError)
Object.const_set :TopErrX, k
p TopErrX.name
begin
  raise TopErrX, "x"
rescue TopErrX => e
  p e.message
end
