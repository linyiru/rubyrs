# Symbol#start_with? / #end_with? — native on the interned name (#380).
def t(label)
  puts "#{label}: #{yield.inspect}"
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message}"
end

# Single argument, hit and miss.
t(:s_hit) { :name=.start_with?("na") }
t(:s_miss) { :name=.start_with?("me") }
t(:e_hit) { :name=.end_with?("=") }
t(:e_miss) { :name.end_with?("=") }
t(:s_whole) { :name.start_with?("name") }
t(:e_longer) { :name.end_with?("xname") }
t(:s_empty) { :name.start_with?("") }
t(:e_empty_sym) { :"".end_with?("") }
t(:e_empty_sym_miss) { :"".end_with?("a") }
t(:operator) { :[]=.end_with?("=") }
t(:predicate) { :empty?.end_with?("?") }

# Multiple arguments: any hit wins; order is irrelevant to the answer.
t(:s_multi) { :action_dispatch.start_with?("x", "y", "action") }
t(:e_multi) { :name=.end_with?("?", "!", "=") }
t(:s_multi_miss) { :name.start_with?("x", "y") }
t(:e_multi_miss) { :name.end_with?("x", "y") }

# No arguments: false.
t(:s_none) { :name.start_with? }
t(:e_none) { :name.end_with? }

# Non-String arguments raise TypeError...
t(:s_int) { :name.start_with?(1) }
t(:e_int) { :name.end_with?(1) }
t(:s_sym) { :name.start_with?(:na) }
t(:e_sym) { :name.end_with?(:me) }
t(:e_nil) { :name.end_with?(nil) }
t(:s_true) { :name.start_with?(true) }
t(:e_regexp) { :name.end_with?(/e/) }
t(:e_array) { :name.end_with?(["e"]) }
# ...but only once reached: a hit before the bad arg returns true.
t(:s_hit_then_bad) { :name.start_with?("n", 1) }
t(:e_miss_then_bad) { :name.end_with?("x", 1) }
t(:e_hit_then_bad) { :name.end_with?("e", nil) }

# Implicit to_str conversion.
class AffixStr; def initialize(s) = @s = s; def to_str = @s; end
class BadAffix; def to_str = 42; end
t(:s_to_str) { :name.start_with?(AffixStr.new("na")) }
t(:e_to_str) { :name.end_with?(AffixStr.new("x"), AffixStr.new("me")) }
t(:e_bad_to_str) { :name.end_with?(BadAffix.new) }
o = Object.new
def o.to_str = "me"
t(:e_singleton_to_str) { :name.end_with?(o) }

# start_with? takes a Regexp anchored at index 0 and sets $~.
t(:s_re_hit) { :name.start_with?(/n./) }
t(:s_re_later) { :name.start_with?(/a/) }
t(:s_re_md) { :name.start_with?(/n(.)/); [$~[0], $1] }
t(:s_re_clears) { "zz" =~ /z/; :name.start_with?(/x/); $~ }
t(:s_str_keeps) { "zz" =~ /z/; :name.start_with?("n"); $~[0] }
t(:s_re_mixed) { :name.start_with?("x", /na/) }

# Non-ASCII symbols: byte-level match on character boundaries only.
t(:u_s_hit) { :"héllo".start_with?("hé") }
t(:u_e_hit) { :"héllo".end_with?("llo") }
t(:u_e_multibyte) { :"日本語".end_with?("語") }
t(:u_s_multibyte) { :"日本語".start_with?("日本") }
t(:u_s_miss) { :"日本語".start_with?("本") }
t(:u_s_partial) { :"é".start_with?("\xC3".force_encoding("UTF-8")) }
t(:u_e_partial) { :"é".end_with?("\xA9".force_encoding("UTF-8")) }
t(:u_s_re) { :"héllo".start_with?(/h./); $~[0] }
t(:u_binary_ascii_arg) { :"héllo".start_with?("h".b) }
t(:u_binary_incompat) { :"é".start_with?("\xC3".b) }
t(:u_e_binary_incompat) { :"é".end_with?("\xA9".b) }
t(:ascii_binary_arg) { :name.start_with?("\xFF".b) }

# The idioms that motivated the fast path.
setters = %i[name= size empty? []= to_s]
p setters.select { |m| m.end_with?("=") }
p setters.map { |m| m.start_with?("to_", "em") }

# respond_to? / method objects / send still see the methods.
p :a.respond_to?(:start_with?), :a.respond_to?(:end_with?)
p Symbol.method_defined?(:start_with?), Symbol.method_defined?(:end_with?)
p :abc.method(:end_with?).call("c")
p :abc.public_send(:start_with?, "a"), :abc.send(:end_with?, "x")

# A user reopen must win over the native arm, including a call site that
# already ran the fast path. Kept last because the reopen is global.
def affix_probe(s) = s.end_with?("=")
p affix_probe(:x=)
class Symbol
  def end_with?(*args) = "user:#{args.inspect}"
end
p affix_probe(:x=), :abc.end_with?("c"), :abc.start_with?("a")
