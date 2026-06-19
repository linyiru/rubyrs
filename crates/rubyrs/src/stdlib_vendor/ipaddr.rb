# Tier 3 (ADR 0019 Part E) — pure-Ruby IPAddr. No VM change: IPv4/IPv6
# addresses are just integers, CIDR is integer bit-masking. Covers the
# common surface (new / include? / === / ==/eql?/hash / to_s / to_i /
# to_range / family / ipv4?/ipv6? / mask / & / |), enough for
# rack-protection's HostAuthorization (`IPAddr.new("0.0.0.0/0")
# .include?(host)` + `rescue IPAddr::InvalidAddressError`).
#
# Divergences from CRuby (documented): no zone-id (`%eth0`) parsing, no
# `::ffff:1.2.3.4` IPv4-mapped special-casing in to_s, no reverse-DNS
# (`#reverse` / `#ip6_arpa`). The address maths is exact.

class IPAddr
  # CRuby hierarchy: Error < ArgumentError; the specific errors < Error.
  class Error < ArgumentError; end
  class InvalidAddressError < Error; end
  class AddressFamilyError < Error; end
  class InvalidPrefixError < Error; end

  # Socket address-family constants (so `#family` matches CRuby without
  # requiring the socket battery).
  IN4MASK = 0xffffffff
  IN6MASK = 0xffffffffffffffffffffffffffffffff
  AF_INET = 2
  AF_INET6 = 10

  attr_reader :family

  # `IPAddr.new("1.2.3.4")` / `"1.2.3.0/24"` / `"10.0.0.0/255.255.255.0"`
  # / `"::1"` / `"2001:db8::/32"` / `"[::1]/64"`.
  def initialize(addr = '::', family = nil)
    # An Integer addr (with explicit family) is the internal/clone form.
    if addr.is_a?(Integer)
      @family = family || AF_INET
      @addr = addr
      @mask_addr = (@family == AF_INET6 ? IN6MASK : IN4MASK)
      return
    end

    s = addr.to_s.strip
    # Strip the optional `[...]` IPv6 brackets.
    s = s[1..-2] if s.start_with?('[') && s.end_with?(']')
    s = s.sub(/\][^\]]*\z/, '') if s.include?(']') # `[::1]/64` already stripped above

    prefix = nil
    if (slash = s.index('/'))
      prefix = s[(slash + 1)..]
      s = s[0...slash]
    end

    if s.include?(':')
      @family = AF_INET6
      @addr = parse_ipv6(s)
      bits = 128
      full = IN6MASK
    elsif s.include?('.')
      @family = AF_INET
      @addr = parse_ipv4(s)
      bits = 32
      full = IN4MASK
    else
      raise InvalidAddressError, "invalid address: #{addr}"
    end

    @mask_addr =
      if prefix.nil? || prefix.empty?
        full
      elsif prefix.include?('.')
        # Dotted netmask (IPv4 only).
        m = parse_ipv4(prefix)
        m
      else
        n = Integer(prefix) rescue (raise InvalidPrefixError, "invalid prefix: #{prefix}")
        raise InvalidPrefixError, "invalid length: #{n}" if n < 0 || n > bits
        full ^ (full >> n)
      end
    # Normalise to the network address.
    @addr &= @mask_addr
  end

  def ipv4?; @family == AF_INET; end
  def ipv6?; @family == AF_INET6; end
  def to_i; @addr; end
  def prefix
    # Number of leading 1-bits in the mask (the CIDR prefix length).
    bits = ipv6? ? 128 : 32
    leading = 0
    (0...bits).each do |i|
      break if (@mask_addr >> (bits - 1 - i)) & 1 == 0
      leading += 1
    end
    leading
  end

  # The masked network address as a fresh IPAddr (CRuby `#mask`/`#&`
  # helpers build on this); `coerce` keeps include?/== terse.
  def coerce_other(other)
    case other
    when IPAddr then other
    when Integer then IPAddr.new(other, @family)
    else IPAddr.new(other.to_s)
    end
  end
  private :coerce_other

  # `include?(other)` — is `other` within this address's network range?
  # `other` may be a String (coerced; an invalid one raises
  # InvalidAddressError) or an IPAddr. Cross-family → false.
  def include?(other)
    other = coerce_other(other)
    return false unless other.family == @family
    (other.to_i & @mask_addr) == @addr
  end
  alias === include?

  def ==(other)
    other = coerce_other(other)
    @family == other.family && @addr == other.to_i && @mask_addr == other.instance_variable_get(:@mask_addr)
  rescue InvalidAddressError, AddressFamilyError
    false
  end
  alias eql? ==

  def hash
    [@addr, @mask_addr, @family].hash
  end

  # Range from the network address to the broadcast address.
  def to_range
    bits = ipv6? ? 128 : 32
    full = ipv6? ? IN6MASK : IN4MASK
    last = @addr | (full ^ @mask_addr)
    IPAddr.new(@addr, @family)..IPAddr.new(last, @family)
  end

  def <=>(other)
    other = coerce_other(other)
    return nil unless other.family == @family
    @addr <=> other.to_i
  rescue InvalidAddressError, AddressFamilyError
    nil
  end
  include Comparable

  def mask(prefixlen)
    bits = ipv6? ? 128 : 32
    full = ipv6? ? IN6MASK : IN4MASK
    n = Integer(prefixlen)
    raise InvalidPrefixError, "invalid length: #{n}" if n < 0 || n > bits
    m = full ^ (full >> n)
    clone = IPAddr.new(@addr & m, @family)
    clone.instance_variable_set(:@mask_addr, m)
    clone
  end

  def &(other)
    IPAddr.new(@addr & coerce_other(other).to_i, @family)
  end

  def |(other)
    IPAddr.new(@addr | coerce_other(other).to_i, @family)
  end

  def to_s
    ipv6? ? format_ipv6(@addr) : format_ipv4(@addr)
  end

  def inspect
    bits = ipv6? ? 128 : 32
    "#<IPAddr: #{ipv6? ? 'IPv6' : 'IPv4'}:#{to_s}/#{format_mask}>"
  end

  private

  def parse_ipv4(s)
    parts = s.split('.', -1)
    raise InvalidAddressError, "invalid address: #{s}" unless parts.length == 4
    val = 0
    parts.each do |p|
      raise InvalidAddressError, "invalid address: #{s}" unless p =~ /\A\d+\z/
      n = p.to_i
      raise InvalidAddressError, "invalid address: #{s}" if n > 255
      val = (val << 8) | n
    end
    val
  end

  def parse_ipv6(s)
    # Split on "::" (at most once) to expand the zero-run.
    if s.include?('::')
      head, tail = s.split('::', 2)
      raise InvalidAddressError, "invalid address: #{s}" if tail.include?('::')
      head_groups = head.empty? ? [] : head.split(':')
      tail_groups = tail.empty? ? [] : tail.split(':')
      fill = 8 - (head_groups.length + tail_groups.length)
      raise InvalidAddressError, "invalid address: #{s}" if fill < 0
      groups = head_groups + Array.new(fill, '0') + tail_groups
    else
      groups = s.split(':')
      raise InvalidAddressError, "invalid address: #{s}" unless groups.length == 8
    end
    val = 0
    groups.each do |g|
      raise InvalidAddressError, "invalid address: #{s}" unless g =~ /\A[0-9a-fA-F]{1,4}\z/
      val = (val << 16) | g.to_i(16)
    end
    val
  end

  def format_ipv4(addr)
    [(addr >> 24) & 0xff, (addr >> 16) & 0xff, (addr >> 8) & 0xff, addr & 0xff].join('.')
  end

  def format_ipv6(addr)
    groups = (0...8).map { |i| (addr >> (16 * (7 - i))) & 0xffff }
    # Collapse the longest run of zero groups to "::" (CRuby canonical).
    best_start = -1; best_len = 0
    i = 0
    while i < 8
      if groups[i] == 0
        j = i
        j += 1 while j < 8 && groups[j] == 0
        if (j - i) > best_len
          best_len = j - i; best_start = i
        end
        i = j
      else
        i += 1
      end
    end
    if best_len > 1
      head = groups[0...best_start].map { |g| g.to_s(16) }.join(':')
      tail = groups[(best_start + best_len)..].map { |g| g.to_s(16) }.join(':')
      "#{head}::#{tail}"
    else
      groups.map { |g| g.to_s(16) }.join(':')
    end
  end

  def format_mask
    prefix
  end
end
