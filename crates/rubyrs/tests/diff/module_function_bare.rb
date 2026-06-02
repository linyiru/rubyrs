# Bare `module_function` — auto-mirrors subsequent defs onto
# the module's singleton class so `Module.method_name(...)`
# resolves at call time. The Tier-1 implementation in
# `vm/dispatch.rs::do_call` previously only flipped the
# class_visibility_stack to Private; the singleton-class
# install was the documented missing piece (`sinatra_jsonp_smoke`
# vendored multi_json shim stumbled into it).

# (1) Headline — bare form, def, then call as module function.
module M1
  module_function
  def hi(x); "hi #{x}"; end
  def bye(x); "bye #{x}"; end
end
puts M1.hi("alice")
puts M1.bye("bob")

# (2) Defs BEFORE module_function don't mirror — the flag is
# forward-looking, not retroactive.
module M2
  def before_mf; "before"; end
  module_function
  def after_mf; "after"; end
end
begin
  puts M2.before_mf
rescue NoMethodError
  puts "before_mf: NoMethodError"
end
puts M2.after_mf

# (3) Explicit-form `module_function :name` is unaffected — it
# still retroactively installs already-defined methods on the
# singleton (vm/dispatch.rs dedicated arm).
module M3
  def x; "x_def"; end
  module_function :x
end
puts M3.x

# (4) Bare form's mirrored copy is PUBLIC on the singleton even
# though the instance entry is PRIVATE. `M.foo` works; `obj
# = Object.new.tap { _1.extend M }; obj.foo` works too because
# the instance-private flip allows implicit-self calls.
module M4
  module_function
  def stamp; "stamped"; end
end
puts M4.stamp

# (5) Multiple bare-form bodies in same script — independent
# state.
module M5
  module_function
  def msg; "from M5"; end
end
module M6
  def msg; "from M6 — not mirrored"; end
end
puts M5.msg
begin
  puts M6.msg
rescue NoMethodError
  puts "M6.msg: NoMethodError (no module_function)"
end
