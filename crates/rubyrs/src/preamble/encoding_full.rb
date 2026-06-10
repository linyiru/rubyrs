## `_encoding_full` registry surface (ADR 0020 Tier 2, v1:
## ISO-8859-1). Loaded at the preamble tail, cfg-gated — without
## the feature none of this exists and Encoding.find serves only
## the core three names.
class Encoding
  ISO_8859_1 = Encoding.new("ISO-8859-1")

  class << self
    alias __rubyrs_find_core find
    ## Layer the registry names over the core resolver. CRuby also
    ## accepts the hyphen-less "ISO8859-1" alias.
    def find(name)
      case name.to_s.upcase
      when "ISO-8859-1", "ISO8859-1" then ISO_8859_1
      else __rubyrs_find_core(name)
      end
    end
  end
end
