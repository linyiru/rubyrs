# String extensions that live above the native core: methods whose
# FULL CRuby semantics need Unicode tables Tier 1 doesn't carry, but
# whose ASCII subset is exact and covers the real require-chains
# (addressable's URI normalization calls `unicode_normalize` on URL
# components, which are ASCII in the supported sites).
class String
  # Byte-level ASCII check (CRuby's is encoding-aware; Tier 1 strings
  # are byte-oriented so the byte scan is the same answer for UTF-8).
  def ascii_only?
    each_byte { |b| return false if b > 127 }
    true
  end

  # Unicode normalization (NFC/NFD/NFKC/NFKD). ASCII strings are
  # fixed points of every normalization form, so returning a copy is
  # exact CRuby behaviour; non-ASCII would need the full UCD
  # decomposition tables — decline loudly rather than return wrong
  # bytes (ADR 0017 Rule 2: never silently-wrong).
  def unicode_normalize(form = :nfc)
    unless %i[nfc nfd nfkc nfkd].include?(form)
      raise ArgumentError, "invalid normalization form #{form}"
    end
    return dup if ascii_only?
    raise NotImplementedError,
          "String#unicode_normalize: non-ASCII input is not supported in the rubyrs subset"
  end

  def unicode_normalized?(form = :nfc)
    unless %i[nfc nfd nfkc nfkd].include?(form)
      raise ArgumentError, "invalid normalization form #{form}"
    end
    return true if ascii_only?
    raise NotImplementedError,
          "String#unicode_normalized?: non-ASCII input is not supported in the rubyrs subset"
  end

  # `String#encode!` — the in-place variant of `#encode`: transcode (or
  # set the encoding) and mutate the receiver, returning self. Built on
  # the native `#encode` + in-place `#replace` so it inherits encode's
  # transcoding + `undef:`/`replace:` option handling. A frozen receiver
  # raises FrozenError via `replace`. Surfaced by bridgetown-core's
  # `ERBView#initialize` (`@buffer.encode!`).
  def encode!(*args)
    replace(encode(*args))
  end
end
