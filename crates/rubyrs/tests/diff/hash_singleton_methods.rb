# Per-instance eigenclass on Hash — the openstruct-over-Hash
# pattern (minitest's ValueMonad tests): def h.method_missing,
# primitive overrides, singleton_class materialization, GC
# survival of define_method captures.
struct = { :_ => "a", :value => "b", :expect => "c" }
def struct.method_missing(k)
  self[k]
end
p struct._
p struct.value
p struct.expect
p struct[:_]
p struct.size
def struct.size; :overridden; end
p struct.size
p struct.singleton_class.is_a?(Class)
p struct.respond_to?(:method_missing)
plain = { x: 1 }
p plain.size
p plain.respond_to?(:method_missing)
# define_method capture survives allocation churn (STRESS_GC food)
payload = "captured"
struct.singleton_class.send(:define_method, :grab) { payload * 2 }
300.times { |i| [i.to_s] }
p struct.grab
