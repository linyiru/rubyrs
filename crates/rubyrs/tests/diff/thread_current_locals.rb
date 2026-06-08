# Thread.current[:key] thread-local variables. In the single-threaded
# model Thread.current IS the Thread class, so these live in one
# process-global store. rouge's Formatter.escape_enabled? reads
# Thread.current[:'rouge/with-escape'].
p Thread.current[:nope]                 # nil (unset)
Thread.current[:x] = 42
p Thread.current[:x]                    # 42
Thread.current[:'ns/flag'] = true
p Thread.current[:'ns/flag']            # true
p Thread.current.key?(:x)               # true
p Thread.current.key?(:nope)            # false
Thread.current[:x] = nil
p Thread.current[:x]                    # nil
p Thread.current.thread_variable_get(:'ns/flag')  # true
Thread.current.thread_variable_set(:y, 7)
p Thread.current[:y]                    # 7
# escape_enabled?-style read-or-default idiom.
p !!(Thread.current[:'ns/flag'])        # true
p !!(Thread.current[:never])            # false
