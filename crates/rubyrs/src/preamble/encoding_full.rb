## `_encoding_full` registry surface (ADR 0020 Tier 2). Loaded at
## the preamble tail, cfg-gated — without the feature none of this
## exists and Encoding.find serves only the core three names.
##
## Registry v2: ISO-8859-1 (hand-written codec) + seven
## encoding_rs-backed entries. Windows-31J carries CRuby's
## SJIS/CP932 aliases — the WHATWG shift_jis table has windows-31j
## semantics, and CRuby's STRICT Shift_JIS is deliberately not
## registered (see encoding_full.rs).
class Encoding
  ISO_8859_1 = Encoding.new("ISO-8859-1")
  Windows_1252 = Encoding.new("Windows-1252")
  ISO_8859_15 = Encoding.new("ISO-8859-15")
  KOI8_R = Encoding.new("KOI8-R")
  Windows_31J = Encoding.new("Windows-31J")
  EUC_JP = Encoding.new("EUC-JP")
  GBK = Encoding.new("GBK")
  Big5 = Encoding.new("Big5")

  class << self
    alias __rubyrs_find_core find
    ## Layer the registry names over the core resolver, with
    ## CRuby's alias folds.
    def find(name)
      case name.to_s.upcase
      when "ISO-8859-1", "ISO8859-1" then ISO_8859_1
      when "WINDOWS-1252", "CP1252" then Windows_1252
      when "ISO-8859-15", "ISO8859-15" then ISO_8859_15
      when "KOI8-R", "KOI8R" then KOI8_R
      when "WINDOWS-31J", "CP932", "SJIS" then Windows_31J
      when "EUC-JP", "EUCJP" then EUC_JP
      when "GBK", "CP936" then GBK
      when "BIG5" then Big5
      else __rubyrs_find_core(name)
      end
    end
  end
end
