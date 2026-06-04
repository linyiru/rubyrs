# MatchData — the value returned by `String#match(regex)`. Wraps
# the whole match + numbered captures + context (pre/post slices
# of the original string, the source string, the regexp used).
# CRuby's MatchData has more surface (named-capture maps,
# offset/begin/end, values_at, named_captures); we expose the
# instance methods sinatra-contrib / Rack / mail-style gems most
# commonly reach for. Stored as a regular user-class so the
# existing instance-method dispatch carries the load.
#
# The Rust side allocates these via `Vm::materialize_match_data`
# (vm/match_data.rs) when `String#match` produces a hit — that
# helper looks the class up by name (`MatchData`), so this file
# must be loaded before any `match` call.
#
# Optional ivars (set when the call site has the data, nil
# otherwise — `String#match("substr")` only populates @whole/@caps,
# leaving the regex-bound surface nil):
#   * `@pre_match`  — String before the match in the original
#   * `@post_match` — String after the match in the original
#   * `@string`    — the original String the regex ran against
#   * `@regexp`    — the Regexp object used for matching

class MatchData
  def initialize(whole, caps, pre_match = nil, post_match = nil, string = nil, regexp = nil, named_caps = nil)
    @whole = whole
    @caps  = caps
    @pre_match = pre_match
    @post_match = post_match
    @string = string
    @regexp = regexp
    @named_caps = named_caps
  end
  # CRuby's MatchData#[] is overloaded:
  #   * Integer (positional, 0 = whole, N = N-th group)
  #   * String or Symbol (named capture lookup)
  # The named-capture lookups consult @named_caps (Hash) which
  # the Rust side populates when the matching Regexp had `(?<name>
  # ...)` groups. Falls through to nil for unknown names —
  # matches CRuby.
  def [](i)
    if i.is_a?(Symbol) || i.is_a?(String)
      key = i.to_s
      if @named_caps && @named_caps.key?(key)
        @named_caps[key]
      else
        raise IndexError, "undefined group name reference: #{key}"
      end
    elsif i == 0
      @whole
    else
      @caps[i - 1]
    end
  end
  def captures
    @caps
  end
  # `named_captures` — returns a Hash mapping each named group's
  # name to its captured String (or nil for groups that didn't
  # participate). Empty Hash when the matching pattern had no
  # named groups. CRuby returns `{}` for both shapes; we
  # likewise return an empty Hash when @named_caps is nil.
  def named_captures
    @named_caps ? @named_caps.dup : {}
  end
  def to_a
    [@whole] + @caps
  end
  def size
    @caps.length + 1
  end
  def length
    size
  end
  def to_s
    @whole
  end
  def pre_match;  @pre_match;  end
  def post_match; @post_match; end
  def string;     @string;     end
  def regexp;     @regexp;     end
  def inspect
    # CRuby format: `#<MatchData "<whole>" 1:"<cap1>" 2:"<cap2>" ...>`.
    # When the regex had no groups, the trailing per-group list
    # is omitted entirely: `#<MatchData "<whole>">`. Non-
    # participating groups (alternation arms that didn't match)
    # serialise as `N:nil` rather than `N:""`. String captures
    # go through `String#inspect` so quotes / escapes match CRuby
    # byte-for-byte.
    parts = "#<MatchData " + @whole.inspect
    i = 0
    while i < @caps.length
      cap = @caps[i]
      parts += " " + (i + 1).to_s + ":" + (cap.nil? ? "nil" : cap.inspect)
      i += 1
    end
    parts + ">"
  end
end
