# MatchData — the value returned by `String#match(regex)`. Wraps
# the whole match + numbered captures. CRuby's MatchData has a
# lot of API surface (`pre_match`, `post_match`, `named_captures`,
# `regexp`); we expose only `[]`, `captures`, `to_a`, `size`,
# `to_s`, and `inspect`. Stored as a regular user-class so the
# existing instance-method dispatch carries the load.
#
# The Rust side allocates these via `Vm::materialize_match_data`
# (vm/match_data.rs) when `String#match` produces a hit — that
# helper looks the class up by name (`MatchData`), so this file
# must be loaded before any `match` call.

class MatchData
  def initialize(whole, caps)
    @whole = whole
    @caps  = caps
  end
  def [](i)
    if i == 0
      @whole
    else
      @caps[i - 1]
    end
  end
  def captures
    @caps
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
  def inspect
    # Plain concatenation — kept simple to avoid quote/hash
    # sequences that conflict with the surrounding Rust raw
    # string delimiter.
    "<MatchData " + @whole + ">"
  end
end
