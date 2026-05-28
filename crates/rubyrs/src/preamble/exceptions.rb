# Built-in exception hierarchy. Loaded by `Runtime::load_preamble`
# before the class stubs / mixin preambles below so that
# `RuntimeError` / `StandardError` / `Exception` etc. resolve when
# user code (or another preamble) raises.
#
# Hierarchy mirrors CRuby's `Init_Exception` (error.c) for the
# error classes the runtime actually raises. The "Tier 1" rubyrs
# subset omits `LoadError`, `SyntaxError`, `EncodingError`,
# `IOError`, `SystemCallError`, `SystemExit`, `Interrupt`,
# `SignalException`, `SecurityError`, `Math::DomainError` etc.;
# those are added as the runtime grows.
#
# Re-opening any of these from user code works (`class
# RuntimeError; def foo; end`), but adding methods that way
# won't change rescue semantics — the `is_a` walk lives on the
# Rust side and reads the `superclass` chain stored here.

class Exception
  def initialize(msg)
    @message = msg
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
