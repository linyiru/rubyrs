# etc — vendored subset (ADR 0026 blessed-reimpl). The real etc is
# a C extension over getpwent/sysconf; the subset here is what
# pure-Ruby gems consult at load time.
#
# Motivating consumer: minitest 5.25 (`require "etc"` at the top of
# minitest.rb; `Etc.nprocessors` sizes its parallel executor), and
# the parallel gem's `processor_count` (rubocop --parallel sizes its
# forked-worker pool with it).
#
# `nprocessors` reports the REAL logical-core count: with the
# fiber-backed cooperative thread scheduler (preamble/thread.rb) and
# real fork(2), worker pools sized to the machine get genuine
# parallelism, exactly like CRuby. (Historically this returned 1 —
# honest for the deferred thread model, but it capped the parallel
# gem at a single forked worker.)
module Etc
  def self.nprocessors
    (__rubyrs_nprocessors rescue 1)
  end
end
