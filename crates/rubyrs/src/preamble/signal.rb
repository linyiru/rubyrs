# ADR 0025 Phase 4a: `Signal` module — `Signal.trap` user-facing
# API. Delegates to the `__rubyrs_signal_trap` Kernel builtin
# (vm/kernel.rs) which stores the handler state in
# `Vm::signal_traps` keyed by Unix signal number.
#
# Two-arg form (CRuby-style):
#   Signal.trap("INT", "DEFAULT")
#   Signal.trap("INT", "IGNORE")
#   Signal.trap("INT", proc { ... })
#
# Block form (also CRuby-style):
#   Signal.trap("INT") { puts "got Ctrl+C"; exit }
#
# Signal name normalization accepts: bare ("INT"), SIG-prefixed
# ("SIGINT"), Symbol (:INT / :SIGINT), Integer (2). See
# `signals::parse_signal_name` for the full Tier-1 portable
# signal subset.
#
# Returns the PREVIOUSLY-installed handler in the same shape:
#   "DEFAULT" string, "IGNORE" string, or a Proc.
module Signal
  def self.trap(sig, handler = nil, &block)
    __rubyrs_signal_trap(sig, handler, block)
  end
end
