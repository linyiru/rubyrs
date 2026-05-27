# `Thread.current.object_id` — Tier 1 single-threaded stub.
#
# Motivating consumer: tilt-2.7.0 `lib/tilt/template.rb:439`
#
#   method_name = "__tilt_#{Thread.current.object_id.abs}"
#
# tilt suffixes the compiled method name with the current thread's
# object_id so different threads can compile templates without
# stomping on each other. rubyrs is single-threaded (ADR 0017
# Tier 1 Rule 4: no OS threads), so any stable integer works —
# we just need `Thread.current.object_id` to dispatch and return
# something Integer-shaped.
#
# Coverage:
#   - `Thread.current` returns a non-nil object
#   - `.object_id` chain dispatches and returns an Integer
#   - `.abs` works on the result
#   - Tilt-shape string interpolation produces a stable name
#   - Calling twice returns the same id (deterministic — tilt
#     relies on this to avoid collisions)
#
# DIVERGENCE (documented in preamble/thread.rb): this is the
# entire Thread API. No `Thread.new`, no `#join`, no `Thread.list`,
# no integration with `Mutex`/`ConditionVariable`. The integer
# value is a constant `1`, not a derived address.

# --- Thread.current dispatches and is non-nil ---
puts Thread.current.nil?                              # false

# --- .object_id is an Integer ---
puts Thread.current.object_id.is_a?(Integer)          # true

# --- .abs works on it ---
puts Thread.current.object_id.abs >= 0                # true

# --- Two calls return the same id (deterministic) ---
a = Thread.current.object_id
b = Thread.current.object_id
puts a == b                                           # true

# --- tilt-shape string interpolation produces a usable name ---
name = "__tilt_#{Thread.current.object_id.abs}"
puts name.start_with?("__tilt_")                      # true
puts name.length > "__tilt_".length                   # true
