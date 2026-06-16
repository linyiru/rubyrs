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
    refresh_byte_addressable
  end

  # The native `__strscan_search` fast path addresses `@str` by BYTE
  # offset, so it's only sound when the scanner's character index
  # equals the byte index — i.e. an ASCII-8BIT buffer OR an ASCII-only
  # string (every char is one byte). Cached because `ascii_only?` is
  # O(n); recomputed only when `@str` is replaced/grown. A multipart
  # body is always one of these two (binary data, or all-ASCII tagged
  # UTF-8 after the empty-buffer + ASCII concat), so this is what keeps
  # `scan_until` linear there; genuine multi-byte UTF-8 (ERB/rouge
  # source) falls back to the slice path.
  def refresh_byte_addressable
    @byte_addressable = @str.encoding == Encoding::BINARY || @str.ascii_only?
  end
  private :refresh_byte_addressable

  def string
    @str
  end

  # `scanner.string = s` — replace the scanned string and reset the
  # scan position to 0 (CRuby). rack's multipart parser rebases its
  # buffer with `@sbuf.string = @sbuf.rest` after consuming a chunk.
  def string=(s)
    @str = s.to_s
    @pos = 0
    @last_md = nil
    @match_pos = nil
    refresh_byte_addressable
    s
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

  # scan/check/skip/match? inline the native anchored match instead of
  # going through `match_at_pos` (+ a `$~[0]` round-trip). kramdown's
  # span parser calls these ~14× per character (≈200k times on a small
  # page under `mark_highlighting`), so each saved Ruby dispatch /
  # method-call frame is multiplied that many times — the native match
  # itself is ~0.18µs, but the old `check → match_at_pos → __strscan…`
  # chain plus `md[0]` cost ~1.2µs. `__strscan_match_at` returns the
  # matched STRING directly (or nil / false-to-fall-back) and sets `$~`.
  def scan(regex)
    if @byte_addressable
      m = @str.__strscan_match_at(regex, @pos)
      unless m == false
        if m.nil?
          @last_md = nil
          return nil
        end
        @last_md = $~
        @match_pos = @pos
        @pos += m.length
        return m
      end
    end
    md = match_at_pos(regex)
    return nil if md.nil?
    matched = md[0]
    @pos += matched.length
    matched
  end

  def check(regex)
    if @byte_addressable
      m = @str.__strscan_match_at(regex, @pos)
      unless m == false
        if m.nil?
          @last_md = nil
          return nil
        end
        @last_md = $~
        @match_pos = @pos
        return m
      end
    end
    md = match_at_pos(regex)
    md.nil? ? nil : md[0]
  end

  def skip(regex)
    matched = scan(regex)
    matched && matched.length
  end

  def match?(regex)
    if @byte_addressable
      m = @str.__strscan_match_at(regex, @pos)
      unless m == false
        if m.nil?
          @last_md = nil
          return nil
        end
        @last_md = $~
        @match_pos = @pos
        return m.length
      end
    end
    md = match_at_pos(regex)
    md.nil? ? nil : md[0].length
  end

  # --- search ahead from the current position ---

  def scan_until(regex)
    consumed = search_from_pos(regex)
    return nil if consumed.nil?
    # `@str[@pos, consumed]` copies only the matched span (O(consumed));
    # the old `rest[0, consumed]` first built `@str[@pos..]` (O(remaining))
    # — the second O(n²) source after the slice in `search_from_pos`.
    result = @str[@pos, consumed]
    @pos += consumed
    result
  end

  def check_until(regex)
    consumed = search_from_pos(regex)
    consumed.nil? ? nil : @str[@pos, consumed]
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
    s = str.to_s
    @str += s
    # Incremental refresh (O(appended), not O(@str)): the grown buffer
    # stays byte-addressable iff it became ASCII-8BIT (any high byte
    # flips the whole concat) or it was addressable and the addition is
    # ASCII-only.
    @byte_addressable =
      @str.encoding == Encoding::BINARY || (@byte_addressable && s.ascii_only?)
    self
  end
  alias_method :<<, :concat

  def inspect
    "#<StringScanner #{eos? ? "fin" : "#{@pos}/#{@str.length}"}>"
  end

  private

  # COLD fallback for the anchored family (scan/check/skip/match?): the
  # hot byte-addressable + Regexp path is inlined in those methods via the
  # native `__strscan_match_at`. This handles the rest — a non-ASCII
  # buffer (genuine multi-byte UTF-8, where char index != byte index) or a
  # non-Regexp arg. The `slice =~ regex` on a non-Regexp raises the same
  # TypeError CRuby does (csv probes `scan("x")` and rescues TypeError to
  # decide STRING_SCANNER_SCAN_ACCEPT_STRING). Returns the MatchData on a
  # hit (so `[]` / `matched` / `pre_match` work), or nil.
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
  #
  # For a BINARY buffer (rack multipart bodies; char index == byte
  # index) the engine searches IN PLACE from `@pos` via the native
  # `__strscan_search` hook — `@str[@pos..]` copies O(remaining) bytes
  # on every call, which turns a multi-part `scan_until` loop into
  # O(n²). `__strscan_search` returns the absolute match start (Integer)
  # and sets `$~`, `nil` for no match, or `false` when there's no byte
  # engine for the pattern (then we fall back to the slice path).
  def search_from_pos(regex)
    if @byte_addressable
      r = @str.__strscan_search(regex, @pos)
      unless r == false
        if r.nil?
          @last_md = nil
          return nil
        end
        @last_md = $~
        @match_pos = r
        return (r - @pos) + $~[0].length
      end
    end
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
