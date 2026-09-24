# Symbol#start_with? / #end_with? are native (#380): the method-table
# edits a Ruby definition used to get for free must still apply. Every
# step mutates Symbol globally, so the order matters: the undef runs
# while the fast path is still warm.
def t(label)
  puts "#{label}: #{yield.inspect}"
rescue NameError => e
  puts "#{label}: #{e.class}"
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message}"
end

# Class and module args convert through their singleton to_str.
class AffixK; def self.to_str = "ab"; end
module AffixM; def self.to_str = "bc"; end
t(:class_to_str) { :abc.start_with?(AffixK) }
t(:module_to_str) { :abc.end_with?(AffixM) }
t(:class_no_to_str) { :abc.end_with?(String) }

# undef_method on Symbol hides the native method, including at a call
# site that already ran the fast path.
def sw(s) = s.start_with?("a")
t(:warm) { sw(:abc) }
class Symbol; undef_method :start_with?; end
t(:undef_call) { :abc.start_with?("a") }
t(:undef_warm_site) { sw(:abc) }
t(:undef_send) { :abc.send(:start_with?, "a") }
t(:undef_public_send) { :abc.public_send(:start_with?, "a") }
t(:undef_block_form) { :abc.start_with?("a") { } }
t(:undef_send_block_form) { :abc.send(:start_with?, "a") { } }
t(:undef_respond_to) { :abc.respond_to?(:start_with?) }
t(:undef_method_defined) { Symbol.method_defined?(:start_with?) }
t(:other_name_still_native) { :abc.end_with?("c") }
# A redefinition after the undef wins over the stale tombstone.
class Symbol; def start_with?(*a) = [:redefined, *a]; end
t(:redefined) { :abc.start_with?("a") }
t(:redefined_site) { sw(:abc) }

# Defs on Object or an included module sit BEHIND Symbol's own method.
class Object; def end_with?(*) = :object; end
t(:object_def) { :abc.end_with?("c") }
module AffixInc; def end_with?(*) = :included; end
class Symbol; include AffixInc; end
t(:include_def) { :abc.end_with?("c") }
# A tombstone above Symbol does not shadow Symbol's own method.
class Object; def length = :object; end
class Object; undef_method :length; end
t(:object_tombstone) { :abc.length }

# A prepended module sits AHEAD of Symbol's own method.
module AffixPre; def end_with?(*a) = [:prepended, *a]; end
class Symbol; prepend AffixPre; end
t(:prepend_def) { :abc.end_with?("c") }
# The same holds for the other primitive classes' native arms.
module AffixPreOther; def upcase = :prepended; def succ = :prepended; def each_byte(*) = :prepended; end
class String; prepend AffixPreOther; end
class Integer; prepend AffixPreOther; end
t(:prepend_string) { "a".upcase }
t(:prepend_integer) { 1.succ }
t(:prepend_block_form) { "ab".each_byte { } }

# The prepend chain is transitive: a prepended module's own prepends
# and includes also sit ahead of the class.
module AffixDeepPre; def size = :deep_prepend; def swapcase = :deep_prepend; end
module AffixDeepInc; def downcase = :deep_include; end
module AffixOuter; prepend AffixDeepPre; include AffixDeepInc; end
class Symbol; prepend AffixOuter; end
class String; prepend AffixOuter; end
t(:transitive_prepend_sym) { :abc.size }
t(:transitive_prepend_str) { "a".swapcase }
t(:transitive_include_str) { "A".downcase }
# An undef on Symbol does not hide a method a prepended module supplies.
class Symbol; undef_method :end_with?; end
t(:prepend_then_undef) { :abc.end_with?("c") }
t(:prepend_then_undef_respond_to) { :abc.respond_to?(:end_with?) }
