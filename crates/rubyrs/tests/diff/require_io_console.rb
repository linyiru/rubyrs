# `require "io/console"` — a lenient load-time stdlib stub (the
# `console` gem requires it for terminal output; its only real method
# use, `IO#winsize`, is on the TTY-only path). The load-time contract
# is just that the require succeeds and returns true the first time,
# false on re-require.
puts(require "io/console")
puts(require "io/console")
puts defined?(IO)
