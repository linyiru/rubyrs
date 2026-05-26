# Regexp#=== (used by case/when) populates the same $~/$1.. side
# channel as =~ and String#match. Regression cover for PR #97
# round-2 review: previously the === arm used `is_match` and skipped
# the side-channel update entirely.

s = "hello world"
case s
when /(\w+) (\w+)/
  puts "case: $1=#{$1} $2=#{$2}"
end

# Direct `re === s` outside case/when — same semantics
hit = /(\d+)/ === "abc 123"
puts "direct hit=#{hit} $1=#{$1}"

# Miss clears prior captures and $~
"seed" =~ /(\w+)/        # populate to make sure the clear is observable
miss = /(z+)/ === "abc"
puts "miss=#{miss} $1=#{$1.inspect} $~=#{$~.inspect}"
