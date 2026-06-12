# raise is an ordinary (private) Kernel method: send-form works,
# an eigenclass override intercepts even the bare keyword form
# (minitest's obj.stub :raise, nil), and Symbol#=~ matches like
# its String form (the Mock blank-slate __-filter).
begin
  send(:raise, ArgumentError, "via-send")
rescue ArgumentError => e
  puts "send: #{e.message}"
end
o = Object.new
begin
  o.send(:raise, "obj-send")
rescue RuntimeError => e
  puts "obj: #{e.message}"
end
clapper = Class.new do
  def fail_clap
    raise
    :clap
  end
end.new
clapper.singleton_class.send(:define_method, :raise) { |*_a| nil }
p clapper.fail_clap
# un-stubbed raise still raises
begin
  Object.new.send(:raise, "still-works")
rescue RuntimeError => e
  puts "plain: #{e.message}"
end
p(:__abc =~ /^__/)
p(:foo =~ /^__/)
