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

  # `Exception#backtrace` — Array<String> of `"file:line:in
  # 'method'"` frames, oldest at the END. `@backtrace` is
  # populated by `Vm::trap_to_exception` when a Trap is rescued;
  # exceptions constructed directly via `RuntimeError.new("...")`
  # carry no backtrace yet (matches CRuby — `raise`d-then-caught
  # carries one, `.new`-but-never-raised returns nil).
  def backtrace
    @backtrace
  end

  # `Exception#full_message` for gem logging paths
  # (rails / sentry-ruby / etc.) that call it without checking
  # `respond_to?(:full_message)` first.
  #
  # CRuby format: `"path:line:in 'method': msg (Class)\n\tfrom
  # ...\n"`. We synthesise the first line from `@backtrace.first`
  # (when present) and chain `\tfrom ...` lines for the rest;
  # `@backtrace == nil` (exception constructed but never raised)
  # falls back to the bare `"msg (Class)\n"` shape.
  #
  # `highlight:` and `order:` are accepted for API-compatibility.
  # Documented divergence: never emits ANSI colour regardless of
  # `highlight: true`. `order: :top` (default for non-tty) lays
  # the head first then `\tfrom` continuations; `order: :bottom`
  # is mapped onto the same `:top` rendering — the CRuby
  # `:bottom` form (numbered `"Traceback (most recent call
  # last)\n\t1: from ..."`) isn't replicated.
  def full_message(highlight: false, order: :top)
    bt = @backtrace
    return "#{@message} (#{self.class})\n" unless bt.is_a?(Array) && !bt.empty?
    head = "#{bt.first}: #{@message} (#{self.class})\n"
    tail = bt[1..].map { |f| "\tfrom #{f}\n" }.join
    head + tail
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
## EOFError — raised by IO read methods when they hit EOF. CRuby
## subclass of IOError. Rack 3 references it in class-body
## `rescue` clauses; no IO support means we never raise it, but
## the constant must resolve.
class EOFError < IOError
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

## `EncodingError` — String/encoding mismatches. CRuby raises
## this (and its subclasses below) from string-to-encoding
## conversion paths. We don't raise it from VM internals
## today; pre-installed so `rescue EncodingError` resolves at
## parse time AND user code can `raise EncodingError, msg`
## explicitly. Gem code that does
## `rescue Encoding::CompatibilityError` (mail, addressable,
## etc.) gets through without a NameError.
class EncodingError < StandardError
end

## `Encoding::*` — four subclasses CRuby exposes for
## encoding-aware operations. Pre-installed empty for the same
## "user code may rescue these even though rubyrs doesn't
## raise them today" reason.
module Encoding
  class CompatibilityError < EncodingError
  end
  class ConverterNotFoundError < EncodingError
  end
  class InvalidByteSequenceError < EncodingError
  end
  class UndefinedConversionError < EncodingError
  end
end

## `Math::DomainError` — raised by Math methods on out-of-domain
## input (`Math.sqrt(-1)`, `Math.log(0)`, etc.). Sits under
## StandardError so a bare `rescue` catches it, matching CRuby.
## Pre-installed so `rescue Math::DomainError` resolves at parse
## time. We don't raise from Math primitives today (they return
## NaN / Infinity), but the class needs to exist for user code
## that explicitly catches.
module Math
  class DomainError < StandardError
  end
end

## `SystemCallError` + `Errno::*` — OS-error hierarchy. CRuby's
## actual table has ~140 platform-specific Errno classes; we
## pre-install the most common ones gems reach for in
## `rescue Errno::ENOENT` etc. patterns. Each class's `#errno`
## attribute would normally return the underlying integer error
## code; rubyrs doesn't raise these from VM code today (no
## file-I/O syscall surface), so the attribute is left nil — the
## class structure is what gem code consumes.
class SystemCallError < StandardError
end
module Errno
  ## File / directory not found.
  class ENOENT < SystemCallError; end
  ## Permission denied.
  class EACCES < SystemCallError; end
  ## File / directory already exists.
  class EEXIST < SystemCallError; end
  ## Not a directory.
  class ENOTDIR < SystemCallError; end
  ## Is a directory (when a file was expected).
  class EISDIR < SystemCallError; end
  ## Invalid argument to a syscall.
  class EINVAL < SystemCallError; end
  ## No space left on device.
  class ENOSPC < SystemCallError; end
  ## Broken pipe.
  class EPIPE < SystemCallError; end
  ## Connection refused.
  class ECONNREFUSED < SystemCallError; end
  ## Connection reset by peer.
  class ECONNRESET < SystemCallError; end
  ## Resource temporarily unavailable — non-blocking IO that
  ## would otherwise block. async / nio4r / net-* retry loops
  ## pattern-match on this to decide whether to retry.
  class EAGAIN < SystemCallError; end
  ## Operation would block — on Linux + Darwin (the platforms
  ## CRuby and rubyrs target), EWOULDBLOCK shares the same
  ## errno integer as EAGAIN, so CRuby aliases the constant to
  ## the same class object. `Errno::EWOULDBLOCK == Errno::EAGAIN`
  ## holds and `rescue Errno::EWOULDBLOCK` catches what was
  ## raised as `Errno::EAGAIN`. Mirror exactly so gems
  ## (eventmachine, em-http-request) that write either name
  ## get the same class.
  EWOULDBLOCK = EAGAIN
  ## Operation timed out. net-http / faraday / rest-client all
  ## wrap underlying socket timeouts and re-raise either this
  ## or a higher-level Timeout::Error.
  class ETIMEDOUT < SystemCallError; end
  ## Interrupted system call — a syscall returned EINTR
  ## because a signal handler fired mid-syscall. Signal-handler
  ## test setups (puma worker, sidekiq) loop on EINTR retries.
  class EINTR < SystemCallError; end
  ## Bad file descriptor — typically a close-after-close or
  ## use-after-close bug. Hard-to-reproduce concurrency bugs
  ## surface here.
  class EBADF < SystemCallError; end
  ## Input/output error — failed read/write at the device
  ## level. Disk-full / hardware-failure surface, distinct from
  ## ENOSPC (logical-quota) and ENOENT (path).
  class EIO < SystemCallError; end
  ## Address already in use — server tried to bind a port the
  ## OS reports as taken. puma / rack / sinatra startup
  ## diagnostics rescue this to print a friendly "port already
  ## in use" message instead of a backtrace.
  class EADDRINUSE < SystemCallError; end
  ## Cannot assign requested address — the bind address isn't
  ## valid on this host (typo, missing interface, IPv6 vs IPv4
  ## mismatch).
  class EADDRNOTAVAIL < SystemCallError; end
  ## No route to host — DNS resolved but routing failed.
  ## net-http surfaces this as a "host unreachable" hint.
  class EHOSTUNREACH < SystemCallError; end
  ## Network unreachable — broader network-layer failure than
  ## EHOSTUNREACH. Sibling under SystemCallError.
  class ENETUNREACH < SystemCallError; end
  ## Operation now in progress — non-blocking connect() returns
  ## this when the TCP handshake hasn't completed yet. async-
  ## style IO loops drive this through a select/poll cycle.
  class EINPROGRESS < SystemCallError; end
  ## Transport endpoint not connected — read/write after socket
  ## close. eventmachine / async-io use this in their state-
  ## machine assertions.
  class ENOTCONN < SystemCallError; end
  ## Too many open files (per-process limit). Worker pools
  ## hitting `ulimit -n` surface this.
  class EMFILE < SystemCallError; end
  ## Too many open files in system (system-wide limit). Rarer
  ## than EMFILE but real on container hosts.
  class ENFILE < SystemCallError; end
  ## Cannot allocate memory — syscall-level alloc failure
  ## distinct from rubyrs's ResourceExhausted (which is the
  ## VM-level cap on heap object count, not a host malloc fail).
  class ENOMEM < SystemCallError; end
end

## `SecurityError` — raised by CRuby when SAFE-level checks
## reject an operation. SAFE is deprecated in 3.x but the
## class still exists; pre-installed so `rescue SecurityError`
## resolves at parse time. Intentionally `< Exception`, NOT
## `< StandardError`: a bare `rescue` clause shouldn't swallow
## a security-policy violation.
class SecurityError < Exception
end

## `NoMemoryError` — raised by CRuby when a heap alloc fails.
## Sits `< Exception` (NOT under StandardError) so allocation
## failures can't be swallowed by bare `rescue`. rubyrs's
## ResourceExhausted covers a similar concept for the
## embedder-configurable heap cap; NoMemoryError is here for
## CRuby-shape rescue parity.
class NoMemoryError < Exception
end
