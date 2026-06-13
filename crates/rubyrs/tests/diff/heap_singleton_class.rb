# `Proc#singleton_class` / `Array#singleton_class` materialise the
# per-instance eigenclass (the heap_singletons side-table — twin of
# the Object/Hash/String arms). class_eval on it installs per-instance
# methods, incl. aliasing a native method. rack's spec_response does
# `body.singleton_class.class_eval { alias << call }` on a Proc body so
# `response.write` can `<<` to it.

# Proc: alias the native #call to #<< on just this instance.
content = []
body = proc { |x| content << x }
body.singleton_class.class_eval { alias << call }
body << "bar"
body << "baz"
p content                       # ["bar", "baz"]

# A different proc is unaffected (per-instance, not on Proc itself).
other = proc { |x| x }
p other.respond_to?(:<<)        # false

# class_eval with def, on a Proc eigenclass.
greeter = proc { "hi" }
greeter.singleton_class.class_eval do
  def shout; call.upcase; end
end
p greeter.shout                 # "HI"

# Array per-instance singleton method via singleton_class.class_eval.
arr = [1, 2, 3]
arr.singleton_class.class_eval do
  def second; self[1]; end
end
p arr.second                    # 2
p [9, 8, 7].respond_to?(:second) # false (only `arr` has it)

# singleton_class is idempotent (same object each call).
p body.singleton_class.equal?(body.singleton_class)  # true
