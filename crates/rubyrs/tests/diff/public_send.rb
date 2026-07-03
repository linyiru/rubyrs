# public_send dispatches a method by name, calling PUBLIC methods
# only (the visibility matrix lives in send_family_visibility.rb).
# Discovery: P3 Jekyll spike — jekyll's LogAdapter forwards via
# writer.public_send(level, …).
class W
  def greet(n); "hi #{n}"; end
end
w = W.new
p w.public_send(:greet, "x")
p w.public_send("greet", "y")
p [1, 2, 3].public_send(:map) { |n| n * 2 }
p "abc".public_send(:upcase)
p({a: 1}.public_send(:fetch, :a))
p 5.public_send(:+, 3)
