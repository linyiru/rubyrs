# `File::WRONLY` / `APPEND` / `CREAT` / etc. — POSIX open(2)
# flag constants exposed on the File class for OR-combining
# into open() flag words.
#
# Discovery: P3 Sinatra spike — logger 1.7 (loaded by the
# Sinatra gem chain) evaluates
#   MODE = File::WRONLY | File::APPEND
#   MODE_TO_OPEN = MODE | File::SHARE_DELETE | File::BINARY
#   MODE_TO_CREATE = MODE_TO_OPEN | File::CREAT | File::EXCL
# at class-body load time (logger/log_device.rb:69). Pre-fix
# this raised
# `NameError: uninitialized constant Logger::LogDevice::File::WRONLY`.
#
# rubyrs's Tier 1 stub doesn't open files with these flags;
# the constants exist purely to make the OR'd expressions
# evaluate. Values mirror Linux POSIX.

# Exact values are OS-dependent (Darwin vs Linux differ on
# APPEND/CREAT/EXCL/TRUNC), so the fixture verifies just that
# every constant resolves to an Integer rather than asserting
# specific bit patterns. The gem code only OR's them — never
# inspects the resulting flag word literally — so the
# rubyrs/CRuby divergence on absolute values doesn't surface.
%i[RDONLY WRONLY RDWR APPEND CREAT EXCL TRUNC NOCTTY NONBLOCK
   SYNC BINARY SHARE_DELETE].each do |sym|
  puts "#{sym}=Integer:#{File.const_get(sym).is_a?(Integer)}"
end

# Shape the logger gem uses: OR'd flag word — the gem just
# passes the result to File.open's mode arg, never inspecting
# the bit pattern. Verify the OR'd value is an Integer; exact
# value differs across OSes and isn't load-bearing for the
# downstream code.
mode = File::WRONLY | File::APPEND
puts "mode_int=#{mode.is_a?(Integer)}"
mode_to_open = mode | File::SHARE_DELETE | File::BINARY
puts "mode_open_int=#{mode_to_open.is_a?(Integer)}"
mode_to_create = mode_to_open | File::CREAT | File::EXCL
puts "mode_create_int=#{mode_to_create.is_a?(Integer)}"

# EOFError exists and inherits from IOError.
puts "eof_super=#{EOFError.superclass}"
puts "eof_is_io=#{EOFError.ancestors.include?(IOError)}"
