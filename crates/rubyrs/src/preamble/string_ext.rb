# String extensions that live above the native core: methods whose
# FULL CRuby semantics need Unicode tables Tier 1 doesn't carry, but
# whose ASCII subset is exact and covers the real require-chains
# (addressable's URI normalization calls `unicode_normalize` on URL
# components, which are ASCII in the supported sites).
class String
  # NB: `String#ascii_only?` is a NATIVE primitive (string.rs) backed by
  # the cached ASCII flag — O(1) after first use. It used to live here as
  # a pure-Ruby `each_byte` scan, but that was O(n) per call and uncached,
  # making kramdown's per-element `current_line_number` (which rebuilds a
  # full-string StringScanner) an O(n²) document parse.

  # Unicode normalization (NFC/NFD/NFKC/NFKD). ASCII strings are fixed
  # points of every normalization form, so returning a copy is exact and
  # needs no tables; non-ASCII routes to the native UCD-backed helper
  # (`unicode-normalization` crate, behind `_encoding_full`), which
  # raises NotImplementedError when that feature is absent.
  def unicode_normalize(form = :nfc)
    unless %i[nfc nfd nfkc nfkd].include?(form)
      raise ArgumentError, "Invalid normalization form #{form}."
    end
    return dup if ascii_only?
    __rubyrs_unicode_normalize(self, form)
  end

  def unicode_normalized?(form = :nfc)
    unless %i[nfc nfd nfkc nfkd].include?(form)
      raise ArgumentError, "Invalid normalization form #{form}."
    end
    return true if ascii_only?
    __rubyrs_unicode_normalized_p(self, form)
  end

  # `String#grapheme_clusters` / `#each_grapheme_cluster` — UAX#29
  # extended grapheme clusters. An ASCII string has one cluster per
  # character (no combining marks possible), handled table-free; a
  # non-ASCII string routes to the native segmenter
  # (`unicode-segmentation` crate, behind `_encoding_full`).
  def grapheme_clusters
    return chars if ascii_only?
    __rubyrs_grapheme_split(self)
  end

  def each_grapheme_cluster(&block)
    return to_enum(:each_grapheme_cluster) unless block_given?
    grapheme_clusters.each(&block)
    self
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
