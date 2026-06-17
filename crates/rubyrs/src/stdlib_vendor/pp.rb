# pp — pretty-printer. `Kernel#pp` is a native builtin (vm/kernel.rs);
# this file adds the `Object#pretty_inspect` / `PP` surface that
# `require "pp"` installs. faraday's logging formatter calls
# `body.pretty_inspect`.
#
# Tier-1 approximation: `pretty_inspect` is the single-line `#inspect`
# plus a trailing newline. CRuby's pp lays out long / deeply-nested
# structures across multiple lines with a column-aware algorithm (via
# PrettyPrint); the subset doesn't model that layout, so wide structures
# print on one line instead of wrapped. For short values the output is
# byte-identical to CRuby (`[1, 2, 3]\n`, `{a: 1}\n`, `"hi"\n`).

class Object
  # Multi-line `#inspect` (here: single line + trailing newline).
  # Uses `self.inspect` (explicit receiver) so it works when self is
  # nil/true/false — a bare implicit-self `inspect` doesn't resolve the
  # builtin on those singletons (the Value::Nil bare-dispatch gap).
  def pretty_inspect
    "#{self.inspect}\n"
  end
end

module PP
  # `PP.pp(obj, out = $>, width = 79)` — append `obj`'s pretty_inspect
  # to `out` and return `out` (CRuby returns the output target, not the
  # object — `Kernel#pp` is the one that returns the object).
  def self.pp(obj, out = $stdout, _width = 79)
    out << obj.pretty_inspect
    out
  end

  # Single-line variant — appends `#inspect` (no trailing newline) and
  # returns the output target.
  def self.singleline_pp(obj, out = $stdout)
    out << obj.inspect
    out
  end
end
