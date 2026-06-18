# Fiber.current — a stable, non-nil object usable as a per-fiber Hash key
# (logger's level_key keys the log level on Fiber.current). In the
# single-fiber model it's one root fiber. (We assert the observable
# key/stability contract, not the concrete class, which is a Fiber in
# CRuby and an opaque root sentinel here.)
p Fiber.current.nil?
p Fiber.current.equal?(Fiber.current)
store = {}
store[Fiber.current] = :level
p store[Fiber.current]
p store.key?(Fiber.current)
