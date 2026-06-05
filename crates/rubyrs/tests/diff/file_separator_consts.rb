# `File::SEPARATOR` / `File::ALT_SEPARATOR` / `File::PATH_SEPARATOR`
# — platform path-separator constants installed on the File
# class. Rack 3 `rack/utils.rb:607` evaluates
#   Regexp.union(*[::File::SEPARATOR, ::File::ALT_SEPARATOR].compact)
# at class-body time; pre-fix this raised
# `NameError: uninitialized constant File::SEPARATOR`.
#
# Discovery: P3 Sinatra spike.
#
# rubyrs uses CRuby's POSIX values: SEPARATOR = "/",
# ALT_SEPARATOR = nil (Windows-only "\\"), PATH_SEPARATOR = ":".

puts "sep=#{File::SEPARATOR.inspect}"
puts "alt=#{File::ALT_SEPARATOR.inspect}"
puts "path=#{File::PATH_SEPARATOR.inspect}"

# Shape the Rack class body uses: `.compact` drops the nil
# ALT_SEPARATOR on POSIX, leaving just SEPARATOR.
parts = [::File::SEPARATOR, ::File::ALT_SEPARATOR].compact
puts "parts=#{parts.inspect}"

# Joining with SEPARATOR is the standard portable shape.
puts ["a", "b", "c"].join(File::SEPARATOR)
