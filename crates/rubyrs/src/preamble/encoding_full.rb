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
  ## Fixed-endianness UTF-16 (hand-rolled transcoder; the BOM-form
  ## "UTF-16" dummy encoding is not registered — it needs decode-time
  ## BOM sniffing, a separate follow-up).
  UTF_16LE = Encoding.new("UTF-16LE")
  UTF_16BE = Encoding.new("UTF-16BE")
  UTF_16 = Encoding.new("UTF-16")

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
      when "UTF-16LE", "UTF16LE" then UTF_16LE
      when "UTF-16BE", "UTF16BE" then UTF_16BE
      when "UTF-16", "UTF16" then UTF_16
      else __rubyrs_find_core(name)
      end
    end

    alias __rubyrs_list_core list
    def list
      __rubyrs_list_core + [
        ISO_8859_1, Windows_1252, ISO_8859_15, KOI8_R,
        Windows_31J, EUC_JP, GBK, Big5, UTF_16LE, UTF_16BE, UTF_16,
      ]
    end

    ## Alias spellings probed against CRuby 3.4.1 (the subset our
    ## find() resolves; CRuby also has CP878/PCK/csWindows31J/
    ## eucJP and the locale/external/filesystem pseudo-names).
    alias __rubyrs_aliases_core aliases
    def aliases
      __rubyrs_aliases_core.merge(
        "ISO8859-1" => "ISO-8859-1",
        "CP1252" => "Windows-1252",
        "ISO8859-15" => "ISO-8859-15",
        "CP932" => "Windows-31J",
        "SJIS" => "Windows-31J",
        "CP936" => "GBK",
      )
    end
  end
end
