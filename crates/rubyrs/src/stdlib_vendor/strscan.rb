# StringScanner — pure-Ruby vendor (subset).
#
# CRuby's strscan is a C extension. We don't ship C exts in the
# embeddable subset, so we vendor a pure-Ruby implementation
# covering the API surface real-world consumers actually touch:
#
#   - `StringScanner.new(string)`
#   - `#eos?`                — exhausted?
#   - `#scan(regex)`         — anchored scan at current pos;
#                              returns matched string or nil and
#                              advances pos on hit
#   - `#[](n)`               — n-th group of the last successful
#                              scan; `[0]` is the whole match
#   - `#rest`                — substring from pos to end
#   - `#pos`                 — current byte/char offset
#   - `#peek(n)`             — next n chars without advancing
#
# Motivating use: MRI's `lib/erb/compiler.rb` builds two scanners
# (Simple / Explicit) on top of `StringScanner.new(@src)` and
# only uses `.eos?`, `.scan(regex)`, and `.[](n)`. Tilt's
# `ERBTemplate#prepare` calls `ERB.new(...).src`, which is the
# compiler entry point. Without this vendor the tilt-render
# chain stalls at `LoadError: cannot load such file -- strscan`.
#
# Divergence vs CRuby's strscan: many less-used methods
# (`check`, `skip`, `match?`, `unscan`, `scan_until`,
# `terminate`, ...) are not implemented. The vendor raises a
# clear `NoMethodError` rather than silently no-op'ing if a
# caller reaches for them.
#
# Anchoring approach: instead of `\G` (not in the regex crate's
# Onigmo-compatible subset), each `#scan` slices `@str[@pos..]`
# and matches with `=~`. A hit is recognised only when the
# match starts at byte 0 of the slice (i.e. `=~` returns 0).
# This is O(slice-len) per scan; ERB templates are small so the
# overhead is acceptable. A future cythonic implementation
# could swap in `regex.match_at(pos)`.

class StringScanner
  def initialize(str)
    @str = str
    @pos = 0
    @last_md = nil
  end

  def eos?
    @pos >= @str.length
  end

  def scan(regex)
    rest = @str[@pos..]
    offset = rest =~ regex
    if offset == 0
      matched = $~[0]
      @last_md = $~
      @pos += matched.length
      matched
    else
      @last_md = nil
      nil
    end
  end

  def [](n)
    return nil if @last_md.nil?
    @last_md[n]
  end

  def rest
    @str[@pos..]
  end

  def pos
    @pos
  end

  def peek(n)
    @str[@pos, n]
  end
end
