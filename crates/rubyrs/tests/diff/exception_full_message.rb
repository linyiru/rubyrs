# Exception#full_message API-contract parity. CRuby renders the
# exception with its backtrace plus optional ANSI colour; rubyrs
# emits just the trap-line shape (`"msg (Class)\n"`) because the
# trap backtrace isn't yet captured into the rescued exception
# object — see the preamble comment. This fixture exercises only
# the parts both runtimes agree on byte-identically:
#
#   * The method exists (no NameError at call time).
#   * Accepts `highlight:` and `order:` keyword args without
#     ArgumentError, on the value shapes gems actually pass.
#   * Returns a String.
#   * Result includes both the exception's message and class.
#
# Properties tested via boolean / class checks rather than direct
# stdout matching so the (non-trivial) byte-divergence in the
# render itself doesn't surface in the diff.

begin
  raise RuntimeError, "ohno"
rescue => e
  s = e.full_message(highlight: false)
  puts "is_string=#{s.is_a?(String)}"
  puts "contains_msg=#{s.include?("ohno")}"
  puts "contains_class=#{s.include?("RuntimeError")}"
end

# Keyword arg combinations gems commonly pass.
begin
  raise StandardError, "kw"
rescue => e
  # default highlight (no arg)
  puts "default_class=#{e.full_message.class}"
  # explicit highlight: false (sentry-ruby / rails logger style)
  puts "hl_false_class=#{e.full_message(highlight: false).class}"
  # explicit order: :top
  puts "order_top_class=#{e.full_message(order: :top).class}"
  # explicit order: :bottom
  puts "order_bot_class=#{e.full_message(order: :bottom).class}"
  # both together
  puts "both_class=#{e.full_message(highlight: false, order: :top).class}"
end

# Custom-subclass exception — full_message must respect the
# subclass's actual class, not collapse to Exception.
class MyAppError < StandardError; end
begin
  raise MyAppError, "domain"
rescue => e
  s = e.full_message(highlight: false)
  puts "subclass_msg=#{s.include?("domain")}"
  puts "subclass_class=#{s.include?("MyAppError")}"
end

# StandardError with no explicit message — the trap line uses
# the class name as message (`@message = self.class.name` in
# the preamble Exception#initialize). full_message should still
# render a non-empty string and include the class.
begin
  raise StandardError
rescue => e
  s = e.full_message(highlight: false)
  puts "no_msg_nonempty=#{!s.empty?}"
  puts "no_msg_contains_class=#{s.include?("StandardError")}"
end
