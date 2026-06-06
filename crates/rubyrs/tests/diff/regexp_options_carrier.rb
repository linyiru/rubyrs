# `Regexp#options` + the `Regexp::IGNORECASE/EXTENDED/MULTILINE`
# constants. This is the first half of regex-flag support: the
# `CompiledRegex` carrier now holds a Ruby flag bitmask and
# exposes it via `#options`, and the flag constants resolve.
#
# Literal-flag THREADING (`/foo/i` actually applying the flag and
# `#options` returning 1) lands in the follow-up; here every
# compiled regexp is still flagless, so `#options` is 0 — which
# is correct for the flagless common case AND `Regexp.new(str)`.
#
# Discovery: P3 Sinatra spike — mustermann/regular.rb:45 does
# `Regexp.new(string).options & Regexp::EXTENDED`, which needs
# both `#options` and the constant to exist.

# Flag constants (CRuby's exact values).
puts "IGNORECASE=#{Regexp::IGNORECASE}"
puts "EXTENDED=#{Regexp::EXTENDED}"
puts "MULTILINE=#{Regexp::MULTILINE}"

# #options on flagless regexps is 0.
puts "lit_opts=#{/foo/.options}"
puts "new_opts=#{Regexp.new("bar").options}"

# The mustermann shape: flagless options & EXTENDED is 0.
puts "mustermann=#{(Regexp.new("x").options & Regexp::EXTENDED)}"
puts "ext_zero=#{(Regexp.new("x").options & Regexp::EXTENDED).zero?}"

# respond_to?(:options) agrees.
puts "respond=#{/foo/.respond_to?(:options)}"

# Reflection round-trips are unchanged (no flag prefix leaks into
# #source / #inspect / #to_s for a flagless regexp).
puts "source=#{/hel.lo/.source}"
puts "inspect=#{/hel.lo/.inspect}"
puts "to_s=#{/hel.lo/.to_s}"

# Matching behaviour is unchanged.
puts "match=#{("hello" =~ /hel.lo/).inspect}"
puts "is_a=#{/x/.is_a?(Regexp)}"
