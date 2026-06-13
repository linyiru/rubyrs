# Seven VM-level behaviours the rack 3.1.10 self-suite
# (test/spec_utils.rb, 64 runs / 290 assertions) exposed — each row
# was an E or F in the dashboard before the fix:
#   1. defined?(recv.m) with a missing-const receiver must be nil
#      (was optimistic "method"; rack picks its pure-Ruby
#      secure_compare branch off this probe)
#   2. `public :name` must re-expose a private method inherited
#      from an included module (module_function's instance copy)
#   3. `def self.x` after a bare `module_function` stays PUBLIC —
#      visibility modes only apply to instance defs
#   4. define_singleton_method on a module installs where
#      `singleton_class.send(:remove_method, ...)` can remove it
#   5. a bare `warn` inside a module-function body dispatches to a
#      user-defined singleton `warn` (capture_warnings idiom), not
#      straight to Kernel#warn
#   6. Hash-subclass `super(){ block }` / `super(default)` reach
#      Hash#initialize semantics (default_proc / default)
#   7. `super` from a respond_to? override reaches
#      Object#respond_to?
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0, 70]}"; end; puts "#{l}: #{r}"; end

# 1 — defined? with explicit receivers
t("defined missing const")  { defined?(NopeConst.some_method) }
t("defined const miss m")   { defined?(Integer.nope_nope) }
t("defined const hit")      { defined?(Integer.to_s) }
probe_obj = Object.new
t("defined lvar hit")       { defined?(probe_obj.frozen?) }
t("defined lvar miss")      { defined?(probe_obj.nope) }
t("defined ivar nil hit")   { defined?(@unset_ivar.to_s) }
t("defined ivar nil miss")  { defined?(@unset_ivar.nope) }

# 2 — module_function visibility + public flip
module MfFlip
  module_function
  def alpha; "A"; end
end
t("mf module call")  { MfFlip.alpha }
t("mf inst private") { begin; Class.new { include MfFlip }.new.alpha; rescue NoMethodError; :private_blocked; end }
t("mf public flip")  { Class.new { include MfFlip; public :alpha }.new.alpha }

# 3 — def self.x after module_function
module MfSelfDef
  module_function
  def self.limit; 42; end
end
t("self-def public") { MfSelfDef.limit }

# 4 — define_singleton_method + remove via singleton_class
module DsmHome; end
DsmHome.define_singleton_method(:warn) { |*a| a }
t("dsm call")   { DsmHome.warn("x", up: 2) }
t("dsm remove") { DsmHome.singleton_class.send(:remove_method, :warn); DsmHome.respond_to?(:warn) }
t("dsm gone")   { begin; DsmHome.singleton_class.send(:remove_method, :warn); rescue NameError; :name_error; end }

# 5 — bare warn routed to the singleton override (rack's
# capture_warnings: define_singleton_method(:warn) on the module,
# module-function bodies call bare `warn msg, uplevel: n`)
module WarnHome
  module_function
  def poke
    warn "from poke", uplevel: 1
    :done
  end
end
captured = []
WarnHome.define_singleton_method(:warn) { |*a| captured << a }
t("warn capture ret") { WarnHome.poke }
t("warn captured")    { captured }
WarnHome.singleton_class.send(:remove_method, :warn)

# 6 — Hash subclass super into Hash#initialize
class HBase < Hash; end
class HViv < HBase
  def initialize(*) super() { |h, k| h[k.to_s] if k.is_a?(Symbol) } end
end
class HDef < HBase
  def initialize(*) super(7) end
end
t("hash super dproc") { h = HViv.new; h["x"] = 1; [h[:x], h.default_proc.is_a?(Proc)] }
t("hash super dflt")  { HDef.new["missing"] }
t("hash super plain") { HViv.new["absent"] }

# 5b — bare fail / warn routed to a CLASS-level user override
# (rack Files defines a private `fail(status, body)` returning a
# response triple; the bare call must dispatch there, not raise)
class FailOverride
  def fail(status, body = nil); [:user_fail, status, body]; end
  def warn(*a); [:user_warn, a]; end
  def go; [fail(404, "x"), warn("hi")]; end
end
t("class fail/warn override") { FailOverride.new.go }
t("plain fail still raises")  { begin; fail "boom"; rescue RuntimeError => e; e.message; end }

# 8 — Regexp encoding-flag constants: accepted by Regexp.new,
# ignored for matching, preserved in #options (rack URLMap builds
# Regexp.new(pattern, Regexp::NOENCODING))
t("regexp noenc")    { Regexp.new("ab.c", Regexp::NOENCODING) =~ "ab1c" }
t("regexp opts mix") { Regexp.new("a$", 4 | 32).options }
t("regexp fixedenc") { Regexp::FIXEDENCODING }

# 7 — super from a respond_to? override
class RtoBase
  def respond_to?(name, include_all = false)
    name == :fake_path ? true : super
  end
  def real; end
  private def hidden; end
end
r = RtoBase.new
t("rto fake")    { r.respond_to?(:fake_path) }
t("rto real")    { r.respond_to?(:real) }
t("rto miss")    { r.respond_to?(:nope) }
t("rto private") { [r.respond_to?(:hidden), r.respond_to?(:hidden, true)] }
