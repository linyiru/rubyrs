# Thread — rubyrs is single-threaded (ADR 0017 Tier 1 Rule 4: no
# OS threads). Real codebases (tilt, sinatra) use
# `Thread.current.object_id` to derive a unique method-name suffix
# when compiling templates so different compiled bodies don't
# clash. With one thread, a stable integer is enough.
#
# Motivating consumer: tilt-2.7.0 `lib/tilt/template.rb:439`
# computes `method_name = "__tilt_#{Thread.current.object_id.abs}"`
# at every `compile_template_method` call. The integer just has
# to be deterministic and call-time-stable; tilt suffixes the
# template's identity into the rest of the bytecode separately.
#
# Tier 1 shape:
#   - `Thread.current` returns the Thread class itself (a stable
#     singleton-shaped object). Avoids needing a real Thread
#     instance type when no thread semantics exist to model.
#   - `Thread.object_id` returns a constant non-zero integer so
#     `.object_id.abs` (CRuby's tilt pattern) produces the same
#     stable, positive value every call. `Object#object_id` isn't
#     implemented globally — defining it just on the Thread class
#     keeps the contract narrow and the surface obviously a stub.
#
# DIVERGENCE: this is the entire Thread API. `Thread.new {...}`,
# `Thread.list`, `#join`, `#value`, `Thread.kill`, the whole
# Mutex/ConditionVariable interplay — all absent. Any code that
# inspects the returned `.current` object beyond `.object_id`
# will surface a NoMethodError. Will revisit if/when Tier 2 ever
# introduces real concurrency.

class Thread
  def self.current
    self
  end
  def self.object_id
    1
  end
end
