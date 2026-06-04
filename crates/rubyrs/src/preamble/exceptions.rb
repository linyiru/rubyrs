# Built-in exception hierarchy. Loaded by `Runtime::load_preamble`
# before the class stubs / mixin preambles below so that
# `RuntimeError` / `StandardError` / `Exception` etc. resolve when
# user code (or another preamble) raises.
#
# Hierarchy mirrors CRuby's `Init_Exception` (error.c) for the
# error classes the runtime actually raises. The "Tier 1" rubyrs
# subset omits `SyntaxError`, `EncodingError`, `SystemCallError`,
# `SystemExit`, `SecurityError`, `Math::DomainError` etc.; those
# are added as the runtime grows.
#
# Re-opening any of these from user code works (`class
# RuntimeError; def foo; end`), but adding methods that way
# won't change rescue semantics — the `is_a` walk lives on the
# Rust side and reads the `superclass` chain stored here.

class Exception
  def initialize(msg = nil)
    # CRuby: `RuntimeError.new` (no explicit message) defaults
    # @message to the class name. `puts RuntimeError.new` then
    # renders as `RuntimeError`, and `inspect` as
    # `#<RuntimeError: RuntimeError>`. Required for the
    # `e1.message == "iteration reached an end"`-style super
    # calls in subclass constructors (`StopIteration` etc.)
    # to keep working: passing nil from a subclass that wants
    # the class-name fallback was previously an ArgumentError.
    @message = msg.nil? ? self.class.name : msg
  end
  def message
    @message
  end
  def to_s
    @message
  end
end
class StandardError < Exception
end
class RuntimeError < StandardError
end
class NoMethodError < StandardError
end
class ArgumentError < StandardError
end
class TypeError < StandardError
end
class NameError < StandardError
end
## ScriptError — CRuby's ancestor for compile/load-time errors
## (NotImplementedError, LoadError, SyntaxError). Subclasses
## inherit from ScriptError → Exception in CRuby, NOT from
## StandardError, so a bare `rescue` (which catches StandardError)
## does NOT catch them. Important: stubbing this as a child of
## StandardError would silently change rescue semantics for
## existing CRuby code that relies on NotImplementedError NOT
## being caught by `rescue` clauses.
class ScriptError < Exception
end
class NotImplementedError < ScriptError
end
## LoadError — CRuby's exception for `require` / `require_relative`
## / `load` failure. `rescue LoadError` is the idiomatic catch
## for "feature not available", and the FS-sandbox cap
## (`Config::allow_filesystem_io: false`) raises this when
## load-class methods are gated off (see vm/gc.rs::check_load_allowed).
class LoadError < ScriptError
end
## IOError — CRuby's exception for File / IO failures.
## `rescue IOError` is the idiomatic catch for FS errors;
## raised by the FS-sandbox cap when File class methods are
## gated off (see vm/gc.rs::check_filesystem_io_allowed).
class IOError < StandardError
end
class IndexError < StandardError
end
class KeyError < IndexError
end
## ADR 0024 Phase A.2: `StopIteration < IndexError`. Raised by
## Ruby iterators when an external `Enumerator#next` reaches
## the end. CRuby's `loop` catches it and returns the
## exception's `#result` attr (nil if unset). Pre-installed
## here so the Phase A.3 `def loop` in object.rb can `rescue
## StopIteration => e; e.result; end` matching CRuby exactly.
##
## The `#result` accessor is the bit Phase A.3 cares about;
## rubyrs's broader Enumerator surface (Lazy chains etc.)
## remains out-of-subset.
class StopIteration < IndexError
  def initialize(msg = nil)
    @result = nil
    super(msg.nil? ? "iteration reached an end" : msg)
  end
  attr_accessor :result
end
class ZeroDivisionError < StandardError
end
## CRuby's RangeError — value out of an expected range. Raised
## by `Integer#chr` on bytes outside `0..255`,
## `Integer#pow(exp, mod)` for negative exponents (the modular
## inverse may not exist; we don't compute it), `Numeric#step` on
## negative step with no end, and user-level `raise RangeError`.
## Sits under StandardError so a bare `rescue` catches it.
class RangeError < StandardError
end
## FloatDomainError — raised for IEEE-754 special values that
## have no Integer representation: `Float::INFINITY.to_i`,
## `Float::NAN.to_i`, divmod with NaN divisor, etc. CRuby places
## it under RangeError so `rescue RangeError` (or a bare
## `rescue`) still catches it; users who care specifically
## about float-vs-other range failures can `rescue FloatDomainError`.
class FloatDomainError < RangeError
end
## LocalJumpError — raised when a control-flow keyword
## (`break` / `next` / `return`) escapes the wrong scope. The
## canonical case is `break` from inside a stored Proc (e.g. a
## Hash default-block or any saved block): the block isn't
## currently being yielded-to from an iterator, so there's no
## loop body to break out of. CRuby raises LocalJumpError;
## rubyrs raises it from the `Hash#[]` / `Hash#dig` default-
## block paths.
class LocalJumpError < StandardError
end
class FrozenError < RuntimeError
end
## Intentionally `< Exception`, NOT `< StandardError`. A bare
## `rescue => e` clause filters on `StandardError` by default,
## so attaching `ResourceExhausted` outside that subtree means
## user scripts cannot accidentally — or deliberately — swallow
## their own fuel / heap / frame trap and keep burning quota.
## CRuby uses the same pattern for `SystemExit` and `Interrupt`.
## See docs/adr/0008-resource-caps-for-untrusted-scripts.md.
class ResourceExhausted < Exception
end

## CRuby's `SystemStackError`, raised when method recursion
## exceeds the default depth limit (~10000 frames). Caught by
## the explicit `rescue SystemStackError` or `rescue Exception`
## forms; intentionally placed `< Exception` (NOT `<
## StandardError`) so a bare `rescue` clause cannot silently
## swallow a runaway recursion — same rationale as
## ResourceExhausted / SignalException. Without this class
## installed, the runtime's depth-limit trap surfaces as a
## generic Exception and `rescue SystemStackError` becomes a
## NameError at parse time, breaking parity with every
## CRuby program that handles stack-blowups explicitly.
class SystemStackError < Exception
end

## CRuby's signal-driven exception hierarchy. Pre-installed here
## (without the underlying signal infrastructure that's tracked by
## ADR 0025) so embedders + scripts can `raise Interrupt`, write
## `rescue Interrupt`, and reason about the class hierarchy today.
## When ADR 0025 phases land, the signal-delivery path raises
## these without any preamble change required.
##
## Intentionally `< Exception`, NOT `< StandardError`. A user's
## bare `rescue` clause must NOT swallow a Ctrl+C interrupt — same
## rationale as ResourceExhausted above. CRuby places SignalException
## and SystemExit directly under Exception for this reason.
## v7 round-3 parity: CRuby's `SignalException.new(msg = nil, signo = nil)`
## takes an optional second arg carrying the Unix signal number,
## exposed as `#signo`. Subclasses (e.g. `Interrupt`) inherit
## the same shape.
class SignalException < Exception
  def initialize(*args)
    case args.length
    when 0
      @signo = nil
      super(self.class.name)
    when 1
      @signo = nil
      super(args[0])
    when 2
      @signo = args[1]
      super(args[0])
    else
      raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 0..2)"
    end
  end
  attr_reader :signo
end
## SIGINT-shaped signals (Ctrl+C in a CLI). CRuby instantiates this
## from the default INT handler. rubyrs's signal-handling capability
## (ADR 0025 Phase 1+) will do the same when the host opts in.
class Interrupt < SignalException
end

## `SystemExit` is the exception raised by `Kernel#exit(status)` to
## trigger a clean shutdown — `ensure` blocks fire, `at_exit`
## handlers run, the unwind reaches the script's outer frame and
## the embedder reads the status. Pre-installed for ADR 0025
## Phase 0.5a; the matching `Kernel#exit` / `exit!` / `abort`
## builtins land in Phase 0.5b.
##
## Intentionally `< Exception`, NOT `< SignalException` despite the
## name overlap. CRuby draws the line because `Kernel#exit` (the
## normal source) is programmatic, not signal-driven —
## `SignalException` is reserved for SIG{TERM,HUP,...} shapes that
## reach the process via an actual OS signal. A bare `rescue`
## clause filters on StandardError, so attaching `SystemExit`
## outside that subtree keeps user code from accidentally
## swallowing its own `exit` call — same security-posture
## rationale as `ResourceExhausted` (ADR 0008).
##
## Constructor accepts the same shapes as CRuby 3.x's `SystemExit.new`:
##   - no args        → status=0, message="SystemExit"
##   - Integer        → status=int, message="exit"
##   - true           → status=0, message="exit"
##   - false          → status=1, message="exit"
##   - nil            → status=0, message="exit"
##   - String         → status=0, message=str
##   - (Integer, msg) → status=int, message=msg
class SystemExit < Exception
  def initialize(*args)
    if args.length == 0
      @status = 0
      # v7 round-3 parity: CRuby's `SystemExit.new` returns
      # an instance with message="exit", not "SystemExit".
      super("exit")
    elsif args.length == 1
      arg = args[0]
      if arg.is_a?(Integer)
        @status = arg
        super("exit")
      elsif arg == true
        @status = 0
        super("exit")
      elsif arg == false
        @status = 1
        super("exit")
      elsif arg.nil?
        @status = 0
        super("exit")
      else
        @status = 0
        super(arg.to_s)
      end
    elsif args.length == 2
      @status = args[0]
      super(args[1])
    else
      raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 0..2)"
    end
  end

  attr_reader :status

  def success?
    @status == 0
  end
end
