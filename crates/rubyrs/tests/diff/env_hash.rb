# ENV — exposed as a Hash snapshot of the process environment.
# Set up via `RUBYRS_TEST_*` keys in the test runner so the
# fixture-vs-CRuby comparison sees the same keys. Mutations
# through ENV[k] = v update the snapshot but NOT the real
# process env (documented divergence).

# Inserts work via Hash#[]=.
ENV["RUBYRS_TEST_A"] = "alpha"
ENV["RUBYRS_TEST_B"] = "beta"

p ENV["RUBYRS_TEST_A"]
p ENV["RUBYRS_TEST_B"]
p ENV["RUBYRS_TEST_MISSING"]

# fetch — returns the value or raises KeyError.
p ENV.fetch("RUBYRS_TEST_A")
p ENV.fetch("RUBYRS_TEST_MISSING", "fallback")

begin
  ENV.fetch("RUBYRS_TEST_NONE")
rescue KeyError => e
  puts "rescued: KeyError"
end

# include? / key? on ENV.
puts ENV.include?("RUBYRS_TEST_A")
puts ENV.include?("RUBYRS_TEST_NOPE")
puts ENV.key?("RUBYRS_TEST_B")

# delete returns the removed value (or nil).
puts ENV.delete("RUBYRS_TEST_A")
puts ENV["RUBYRS_TEST_A"].inspect

# ENV behaves like any Hash for iteration over the keys we set.
keys = []
["RUBYRS_TEST_B", "RUBYRS_TEST_C"].each do |k|
  ENV[k] = "x"
  keys << k
end
keys.each { |k| puts "#{k}=#{ENV[k]}" }

# Reading uses Hash semantics; missing key returns nil.
p ENV["truly missing"]

# Inside a method that takes ENV as input.
def lookup(name)
  ENV.fetch(name, "default")
end

ENV["LOOKUP_KEY"] = "found"
puts lookup("LOOKUP_KEY")
puts lookup("OTHER")
