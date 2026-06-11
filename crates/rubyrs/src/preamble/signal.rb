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

module Signal
  # Signal name → number table. EXACTLY the set
  # `signals::parse_signal_name` (the trap plumbing) accepts, plus
  # EXIT — gems use this table as the capability probe before
  # calling trap (minitest checks SIGNALS["INFO"] and skips its
  # info handler when absent), so listing more than trap supports
  # turns the probe into a trap-time ArgumentError. Numbers are
  # Linux-flavored POSIX, same as the parser.
  def self.list
    {
      "EXIT" => 0, "HUP" => 1, "INT" => 2, "QUIT" => 3, "ILL" => 4,
      "TRAP" => 5, "ABRT" => 6, "FPE" => 8, "KILL" => 9,
      "USR1" => 10, "SEGV" => 11, "USR2" => 12, "PIPE" => 13,
      "ALRM" => 14, "TERM" => 15, "CHLD" => 17, "CONT" => 18,
      "STOP" => 19, "TSTP" => 20, "TTIN" => 21, "TTOU" => 22,
      "URG" => 23, "WINCH" => 28,
    }
  end
end


# Kernel-level bare `trap` — same surface as `Signal.trap` (CRuby
# defines both; minitest's `on_signal` uses the bare form from a
# class-method context). Top-level def so the no-recv dispatch
# reaches it from any self.
def trap(*args, &block)
  Signal.trap(*args, &block)
end
