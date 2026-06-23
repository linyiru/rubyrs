# Pure-Ruby veneer over the `RubyrsDigest` host primitive, modelling
# the slice of the `digest` stdlib that ecosystem code reaches for:
# `Digest::SHA2 / SHA256 / SHA1 / MD5` with the class-level
# `hexdigest` / `digest` shortcuts and the incremental
# `new.update(...).hexdigest` surface.
#
# The actual hashing lives in Rust (`crate::digest`); each concrete
# class just carries its lowercase algorithm tag. Byte-exact with
# the CRuby `digest` C extension — see the parity fixtures.
module Digest
  # Shared surface; subclasses override `self.algorithm_tag` with the
  # tag the `RubyrsDigest` host primitive understands.
  class RubyrsBase
    def self.hexdigest(data)
      RubyrsDigest.hexdigest(algorithm_tag, data.to_s)
    end

    def self.digest(data)
      RubyrsDigest.digest(algorithm_tag, data.to_s)
    end

    def initialize
      @buffer = String.new
    end

    def update(data)
      @buffer << data.to_s
      self
    end
    alias_method :<<, :update

    def reset
      @buffer = String.new
      self
    end

    def hexdigest
      self.class.hexdigest(@buffer)
    end

    def digest
      self.class.digest(@buffer)
    end

    def to_s
      hexdigest
    end

    def inspect
      "#<#{self.class.name}: #{hexdigest}>"
    end
  end

  class MD5 < RubyrsBase
    def self.algorithm_tag
      "md5"
    end
  end

  class SHA1 < RubyrsBase
    def self.algorithm_tag
      "sha1"
    end
  end

  class SHA256 < RubyrsBase
    def self.algorithm_tag
      "sha256"
    end
  end

  class SHA512 < RubyrsBase
    def self.algorithm_tag
      "sha512"
    end
  end

  class SHA384 < RubyrsBase
    def self.algorithm_tag
      "sha384"
    end
  end

  # CRuby's `Digest::SHA2` selects a bit length at construction and
  # defaults to the 256-bit variant. rubyrs models the 256 case (the
  # 384/512 selector is not plumbed); this is the shape jekyll's
  # cache keys use — `Digest::SHA2.hexdigest(key)`.
  class SHA2 < RubyrsBase
    def self.algorithm_tag
      "sha256"
    end
  end
end
