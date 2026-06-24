# GC.stat — rubyrs reports all-zero counters (no real collector), but the
# shape matches CRuby so probes resolve: a Hash from the no-arg form with
# the documented keys, an Integer from the single-key form, and the
# in-place Hash-fill form. ActiveSupport::Notifications measures
# GC.stat(:total_allocated_objects) around instrumented events.
s = GC.stat
p s.is_a?(Hash)
p s.key?(:total_allocated_objects)
p s.key?(:count)
p GC.stat(:total_allocated_objects).is_a?(Integer)
p GC.stat(:count) >= 0
h = {}
GC.stat(h)
p h.key?(:count)
p GC.start.nil?
