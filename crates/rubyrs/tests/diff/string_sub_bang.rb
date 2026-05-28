## `String#sub!` — in-place substitution. Closes TRY_RUNS
## pass-10 layer #3 — tilt-2.7.0/lib/tilt/mapping.rb:70 does
##   pattern.sub!(/\A[^.]*\.?/, '')
## inside `Mapping#split` to strip leading filename extensions.
## CRuby returns self on change, nil on no-match (lets the
## surrounding `until registered?(pattern)` loop terminate
## cleanly).

## Shape 1: literal-string pattern, hit → mutates and returns self.
s = "hello world"
ret = s.sub!("world", "there")
puts "lit-hit-ret-eq-self=#{ret.equal?(s)}"
puts "lit-hit-content=#{s}"

## Shape 2: literal-string pattern, miss → returns nil, unchanged.
s = "hello world"
ret = s.sub!("xyz", "abc")
puts "lit-miss-ret=#{ret.inspect}"
puts "lit-miss-content=#{s}"

## Shape 3: regex pattern, hit — the tilt idiom.
s = "foo.bar.erb"
ret = s.sub!(/\A[^.]*\.?/, '')
puts "regex-hit-ret-eq-self=#{ret.equal?(s)}"
puts "regex-hit-content=#{s}"

## Shape 4: regex pattern, miss → nil.
s = "abc"
ret = s.sub!(/xyz/, '')
puts "regex-miss-ret=#{ret.inspect}"
puts "regex-miss-content=#{s}"

## Shape 5: tilt's full split loop — strip leading extensions
## until reaching a terminal. Pin the iteration shape used by
## `Mapping#split`.
pattern = "views/index.html.erb"
3.times do |i|
  result = pattern.sub!(/\A[^.]*\.?/, '')
  puts "step-#{i}: pattern=#{pattern.inspect}, ret=#{result.inspect}"
  break if result.nil?
end

## Shape 6: respond_to?
puts "respond=#{"".respond_to?(:sub!)}"

## Shape 7: aliasing — sub! returns self on hit, which means
## chained calls work. (Less common but worth pinning.)
s = "abc"
s.sub!("a", "A").sub!("b", "B")
puts "chained=#{s}"

## Shape 8: empty pattern — CRuby's `s.sub!("", "X")` prepends X.
s = "abc"
s.sub!("", "X")
puts "empty-pat=#{s}"

## Shape 9: frozen string raises FrozenError.
s = "frozen".freeze
err = begin
  s.sub!("f", "F")
  "no-raise"
rescue FrozenError
  "FrozenError"
end
puts "frozen=#{err}"
