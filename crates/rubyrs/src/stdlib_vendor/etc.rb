# etc — vendored subset (ADR 0026 blessed-reimpl). The real etc is
# a C extension over getpwent/sysconf; the subset here is what
# pure-Ruby gems consult at load time.
#
# Motivating consumer: minitest 5.25 (`require "etc"` at the top of
# minitest.rb; `Etc.nprocessors` sizes its parallel executor).
# rubyrs is single-threaded, so 1 is both honest and the value that
# keeps consumers from spawning pointless workers.
module Etc
  def self.nprocessors
    1
  end
end
