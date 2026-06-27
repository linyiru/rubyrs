# A String subclass instance dispatches method_missing / respond_to_missing?
# like any object (rubyrs used to skip the fallback for String-backed values,
# treating them as plain String). Bridgetown's QuestionableString does
# `env.production?` ⟺ `self == "production"` this way.
class QuestionableString < String
  def method_missing(name, *args)
    s = name.to_s
    return (self == s[0..-2]) if s.end_with?("?")
    super
  end
  def respond_to_missing?(name, include_private = false)
    name.to_s.end_with?("?") || super
  end
end
e = QuestionableString.new("development")
p e.class               # QuestionableString
p e.development?         # true
p e.production?          # false
p e.respond_to?(:test?)  # true
p e.upcase               # "DEVELOPMENT" (String primitives still work)
begin
  e.totally_unknown_no_q
rescue NoMethodError => err
  puts "NoMethodError"    # super → real NoMethodError for non-? misses
end
