# Tier 3 pure-Ruby Pathname — subset matched to CRuby's
# stdlib/pathname.rb output for the deterministic, fs-free
# methods (path-string manipulation only). Methods that touch
# the filesystem (exist?, read, children, ...) are NOT modelled;
# scripts that need them get NoMethodError, which is the right
# "feature absent" surface for the embedding case.
#
# Gated behind the `stdlib` Cargo feature (see ADR 0017 row 125
# and the feature description in `crates/rubyrs/Cargo.toml`).
# Default builds do NOT include this file's behaviour.

class Pathname
  def initialize(path)
    case path
    when Pathname
      @path = path.to_s.dup
    when String
      @path = path.dup
    else
      raise TypeError, "no implicit conversion of #{path.class} into String"
    end
  end

  def to_s
    @path.dup
  end
  alias_method :to_path, :to_s
  # Note: CRuby's Pathname does NOT define `to_str` — Pathname
  # is not a String and doesn't implicitly convert. Keep the
  # vendored subset aligned.

  def inspect
    "#<Pathname:#{@path}>"
  end

  def ==(other)
    other.is_a?(Pathname) && to_s == other.to_s
  end
  alias_method :eql?, :==

  def hash
    @path.hash
  end

  def +(other)
    other_str = other.is_a?(Pathname) ? other.to_s : other.to_s
    if other_str.start_with?("/")
      Pathname.new(other_str)
    elsif @path.empty?
      Pathname.new(other_str)
    elsif @path.end_with?("/")
      Pathname.new(@path + other_str)
    else
      Pathname.new(@path + "/" + other_str)
    end
  end

  def basename
    Pathname.new(File.basename(@path))
  end

  def dirname
    Pathname.new(File.dirname(@path))
  end

  def parent
    dirname
  end

  def extname
    File.extname(@path)
  end

  def absolute?
    @path.start_with?("/")
  end

  def relative?
    !absolute?
  end

  def empty?
    @path.empty?
  end

  # Yield `self` then each ancestor, stripping one trailing path
  # component per step, until the root ("/" for absolute paths) or
  # the first relative component is reached — matching CRuby's
  # `Pathname#ascend`. The block form returns nil; without a block
  # CRuby returns an Enumerator (rubyrs returns the Array of
  # Pathnames, which supports the common `.each`/`.to_a`/`.map`
  # uses). Discovery: P3 Jekyll spike — `site.rb#ensure_not_in_dest`
  # walks `Pathname.new(source).ascend` checking against dest.
  def ascend
    paths = [Pathname.new(@path)]
    path = @path
    loop do
      parent = File.dirname(path)
      # Stop at the root ("/" is its own dirname) or once a relative
      # path is exhausted — CRuby's File.dirname yields "." there, but
      # rubyrs's can yield "" for a no-separator name, so guard both.
      break if parent == path || parent == "." || parent.empty?
      paths << Pathname.new(parent)
      path = parent
    end
    if block_given?
      paths.each { |p| yield p }
      nil
    else
      paths
    end
  end
end
