# throw/catch is control flow (RubyrsThrowSignal) — its backtrace is
# never read, so the unwind skips materializing it. Verify throw/catch
# still behaves exactly, AND that REAL raises still carry a full
# backtrace (the skip must be throw-only).
p catch(:x) { throw :x, 42 }
p catch(:x) { 99 }
p catch(:x) { catch(:y) { throw :x, :outer }; :inner }
p catch(:t) { throw :t }
# real raise keeps its backtrace (non-empty, points at this file)
begin; raise "boom"; rescue => e; p e.message; p(e.backtrace.is_a?(Array) && !e.backtrace.empty?); end
begin; raise ArgumentError, "bad"; rescue => e; p e.backtrace.first.include?("throw_no_backtrace.rb"); end
# uncaught throw surfaces as UncaughtThrowError
begin; throw :nope, 1; rescue UncaughtThrowError; p :uncaught; rescue => e; p [:other, e.class.to_s]; end
# nested catch with rescue inside (throw transparent to ordinary rescue)
p catch(:done) { begin; throw :done, :ok; rescue => e; :swallowed; end }
