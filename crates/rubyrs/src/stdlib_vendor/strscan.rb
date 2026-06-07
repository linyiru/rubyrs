# StringScanner — pure-Ruby vendor (subset).
#
# CRuby's strscan is a C extension. We don't ship C exts in the
# embeddable subset, so we vendor a pure-Ruby implementation
# covering the API surface real-world consumers actually touch:
#
#   - `StringScanner.new(string)`
#   - position: `#pos` / `#pos=` / `#reset` / `#terminate`
#   - anchored at pos: `#scan` / `#check` / `#skip` / `#match?`
#   - search ahead: `#scan_until` / `#check_until` / `#exist?`
#   - last-match queries: `#[]` / `#matched` / `#matched?` /
#     `#pre_match` / `#post_match`
#   - inspection: `#eos?` / `#rest` / `#rest_size` / `#string` /
#     `#peek` / `#beginning_of_line?` (`#bol?`)
#   - char step: `#getch`
#   - grow: `#<<` / `#concat`
#
# Motivating uses: MRI's `lib/erb/compiler.rb` (tilt ERB), rexml's
# legacy `#check` refinement gate, and — the deep consumer — kramdown,
# whose block/span parsers drive everything through a
# `Kramdown::Utils::StringScanner` subclass: `check`, `scan`, `skip`,
# `scan_until`, `pos`/`pos=`, `pre_match`, `matched`, `[]`, `eos?`.
#
# Anchoring approach: instead of `\G` (not in the regex crate's
# Onigmo-compatible subset), each scan slices `@str[@pos..]` and
# matches with `=~`. An anchored hit is recognised only when the
# match starts at byte 0 of the slice (`=~` returns 0); the
# `*_until` family accepts a match at any offset. This is
# O(slice-len) per call; acceptable for the document sizes the
# subset targets.

class StringScanner
  # Version gate consumed by libraries that branch on the strscan
  # API level (rexml: `if StringScanner::Version < "1.0.0"` to decide
  # whether to install a `#check`-on-String refinement). Reporting a
  # modern version makes those legacy-compat refinement paths no-ops.
  Version = "3.1.0"

  def initialize(str)
    @str = str.to_s
    @pos = 0
    @last_md = nil
    # Start offset (into @str) of the last successful match, so
    # pre_match / post_match can be reconstructed.
    @match_pos = nil
  end

  def string
    @str
  end

  def pos
    @pos
  end

  def pos=(n)
    n += @str.length if n < 0
    # CRuby raises RangeError when the resulting position falls outside
    # 0..length, rather than silently storing a degenerate @pos.
    raise RangeError, "index out of range" if n < 0 || n > @str.length
    @pos = n
    n
  end
  alias_method :pointer, :pos
  alias_method :pointer=, :pos=

  def reset
    @pos = 0
    @last_md = nil
    @match_pos = nil
    self
  end

  def terminate
    @pos = @str.length
    @last_md = nil
    @match_pos = nil
    self
  end
  alias_method :clear, :terminate

  def eos?
    @pos >= @str.length
  end

  def rest
    @str[@pos..] || ""
  end

  def rest_size
    rest.length
  end
  alias_method :restsize, :rest_size

  def peek(n)
    @str[@pos, n] || ""
  end
  alias_method :peep, :peek

  def beginning_of_line?
    return true if @pos == 0
    return nil if @pos > @str.length
    @str[@pos - 1] == "\n"
  end
  alias_method :bol?, :beginning_of_line?

  # --- anchored at the current position ---

  def scan(regex)
    md = match_at_pos(regex)
    return nil if md.nil?
    matched = md[0]
    @pos += matched.length
    matched
  end

  def check(regex)
    md = match_at_pos(regex)
    md.nil? ? nil : md[0]
  end

  def skip(regex)
    md = match_at_pos(regex)
    return nil if md.nil?
    @pos += md[0].length
    md[0].length
  end

  def match?(regex)
    md = match_at_pos(regex)
    md.nil? ? nil : md[0].length
  end

  # --- search ahead from the current position ---

  def scan_until(regex)
    consumed = search_from_pos(regex)
    return nil if consumed.nil?
    result = rest[0, consumed]
    @pos += consumed
    result
  end

  def check_until(regex)
    consumed = search_from_pos(regex)
    consumed.nil? ? nil : rest[0, consumed]
  end

  def exist?(regex)
    search_from_pos(regex)
  end

  def skip_until(regex)
    consumed = search_from_pos(regex)
    return nil if consumed.nil?
    @pos += consumed
    consumed
  end

  # --- single character ---

  def getch
    return nil if eos?
    ch = @str[@pos]
    # CRuby's getch SETS the match register to the consumed character
    # (so #matched / #[0] / #post_match work), rather than clearing it.
    @match_pos = @pos
    @last_md = Regexp.new(Regexp.escape(ch)).match(ch)
    @pos += 1
    ch
  end

  # --- last-match queries ---

  def [](n)
    return nil if @last_md.nil?
    @last_md[n]
  end

  def matched
    @last_md && @last_md[0]
  end

  def matched?
    !@last_md.nil?
  end

  def matched_size
    @last_md && @last_md[0].length
  end

  def pre_match
    return nil if @match_pos.nil?
    @str[0, @match_pos]
  end

  def post_match
    return nil if @last_md.nil? || @match_pos.nil?
    @str[(@match_pos + @last_md[0].length)..] || ""
  end

  # --- grow ---

  def concat(str)
    @str += str.to_s
    self
  end
  alias_method :<<, :concat

  def inspect
    "#<StringScanner #{eos? ? "fin" : "#{@pos}/#{@str.length}"}>"
  end

  private

  # Try `regex` anchored at the current position. On a hit, records
  # the MatchData (so `[]` / `matched` / `pre_match` work) and
  # returns it; otherwise clears the last match and returns nil.
  def match_at_pos(regex)
    slice = @str[@pos..] || ""
    if (slice =~ regex) == 0
      @last_md = $~
      @match_pos = @pos
      @last_md
    else
      @last_md = nil
      nil
    end
  end

  # Search for `regex` anywhere at/after the current position. On a
  # hit, records the MatchData and returns the number of characters
  # from the current position through the END of the match (what
  # `scan_until` consumes); otherwise returns nil.
  def search_from_pos(regex)
    slice = @str[@pos..] || ""
    offset = slice =~ regex
    if offset.nil?
      @last_md = nil
      nil
    else
      @last_md = $~
      @match_pos = @pos + offset
      offset + $~[0].length
    end
  end
end
