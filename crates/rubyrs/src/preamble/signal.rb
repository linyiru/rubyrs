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
  # v7 round-3 review surfaced two parity gaps fixed here:
  #
  # 1. `Signal.trap(sig)` (no second arg) is QUERY mode —
  #    returns the current handler without installing one. CRuby
  #    treats `Signal.trap(sig, nil)` as `IGNORE`. Without
  #    distinguishing "arg not given" from "arg is nil", we
  #    can't honor both. v7 splats to disambiguate: 1-arg form
  #    routes through a sentinel Symbol so the host fn knows
  #    it's a query; 2-arg form passes whatever the user gave
  #    (including explicit nil → IGNORE).
  #
  # 2. `Signal.trap(SIGKILL, ...)` / `(SIGSTOP, ...)` is REJECTED
  #    by the host fn (after this v7 sig validation pass).
  #    CRuby's behavior — these signals can't be trapped at the
  #    kernel level.
  def self.trap(*args, &block)
    case args.length
    when 1
      # `Signal.trap(sig) { block }` — install block.
      # `Signal.trap(sig)` alone — QUERY mode (sentinel
      # Symbol so the host fn distinguishes from explicit nil
      # which means IGNORE per CRuby).
      handler = block || :__rubyrs_query_mode__
      __rubyrs_signal_trap(args[0], handler)
    when 2
      # `Signal.trap(sig, handler)` — block (if any) is ignored
      # per CRuby. Handler can be a Proc, "DEFAULT"/"IGNORE"
      # strings, their Symbol equivalents, or nil (CRuby 3.x
      # treats explicit nil as IGNORE).
      __rubyrs_signal_trap(args[0], args[1])
    else
      raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 1..2)"
    end
  end
end
