# Key contract of the fiber-local store (`Thread.current[..]`) and the
# thread-variable store: a String key is interned to a Symbol, anything
# else is a TypeError, `[k] = nil` deletes, and `thread_variable?` is
# false for a nil value. Symbol-key reads and non-nil writes are served
# natively (vm/thread.rs); the rest runs preamble/thread.rb.
t = Thread.current
p [Thread.main.equal?(t), Thread.current.equal?(Thread.current)]
t[:a] = 1
t["s"] = 1
p [t[:s], t["s"], t.key?(:s), t.key?("s")]
t[:n] = 5
p(t[:n] = nil)
p [t.key?(:n), t[:n], t.keys.include?(:n)]
p((t[1] rescue [$!.class, $!.message]))
begin; t[1] = 2; rescue => e; p [e.class, e.message]; end
p((t.key?(1) rescue [$!.class, $!.message]))
p((t[Object.new] rescue $!.class))
p t[:missing]
p(t[:x] = 7)
p t.keys.sort
i = 0
while i < 3
  Thread.current[:loop] = i
  i += 1
end
p Thread.current[:loop]

t.thread_variable_set("v", 1)
p [t.thread_variable_get(:v), t.thread_variable?("v")]
t.thread_variable_set(:nilv, nil)
p [t.thread_variable?(:nilv), t.thread_variables.include?(:nilv)]
p((t.thread_variable_get(1) rescue $!.class))
p [t[:v], t.thread_variable_get(:a)]
