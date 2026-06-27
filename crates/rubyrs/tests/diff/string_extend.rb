# Object#extend works on String receivers (singleton module mix-in) — Sinatra's
# render does `output.extend(...)` on a rendered String. is_a? sees the module;
# String primitives still dispatch. Immediate values can't be extended.
module Shoutable
  def shout = upcase + "!"
end
s = "hello"
result = s.extend(Shoutable)
p result.equal?(s)      # true (returns the receiver)
p s.shout               # "HELLO!"
p s.is_a?(Shoutable)    # true
p s.kind_of?(Shoutable) # true
p s.instance_of?(String)# true (extend doesn't change the real class)
p s.upcase              # "HELLO" (primitives intact)
p s.length              # 5
[5, :sym, 1.5].each do |v|
  begin
    v.extend(Shoutable)
    puts "#{v.class}: no error"
  rescue TypeError => e
    puts "#{v.class}: TypeError"
  end
end
begin; "x".extend; rescue ArgumentError; puts "ArgumentError"; end
