# Same-process Marshal round-trip contract (rubyrs: registry token;
# CRuby: byte stream — both satisfy load(dump(x)) == x and raise
# TypeError on un-dumpable shapes). Deliberately avoids equal?
# (CRuby deep-copies, rubyrs returns the same object — documented
# divergence) and avoids printing the dump payload itself.
h = { "a" => [1, 2], "b" => "x" }
p Marshal.load(Marshal.dump(h)) == h
p Marshal.load(Marshal.dump(nil)).nil?
p Marshal.load(Marshal.dump(42))
p Marshal.load(Marshal.dump([:sym, 1.5]))
begin
  Marshal.dump(proc {})
rescue TypeError
  puts "proc: TypeError"
end
anon = Class.new(StandardError)
begin
  Marshal.dump(anon.new("x"))
rescue TypeError
  puts "anon: TypeError"
end
o = Object.new
def o.zing; end
begin
  Marshal.dump(o)
rescue TypeError
  puts "singleton: TypeError"
end
e2 = RuntimeError.new("x")
e2.instance_variable_set(:@p, proc { 1 })
begin
  Marshal.dump(e2)
rescue TypeError
  puts "nested-proc: TypeError"
end
begin
  Marshal.load("garbage")
rescue TypeError
  puts "garbage-load: TypeError"
end
# Exception reflection: message/backtrace live outside ivars
e = RuntimeError.new("x")
p e.instance_variables
e.instance_variable_set(:@custom, 1)
p e.instance_variables
