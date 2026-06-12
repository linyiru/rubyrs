# Math over the __rubyrs_math host primitive — real singleton-table
# methods (Math.stub :log10 can alias them).
p Math.sqrt(9)
p Math.log10(1000)
p Math.log(Math::E)
p Math.log(8, 2)
p Math.log2(8)
p Math.sin(0)
p Math.cos(0)
p Math.atan2(1, 1) == Math::PI / 4
p Math.hypot(3, 4)
p Math.cbrt(27)
p Math.exp(0)
p Math::PI
p Math::E
begin
  Math.sqrt(-1)
rescue Math::DomainError
  puts "sqrt-domain: ok"
end
begin
  Math.log(-1)
rescue Math::DomainError
  puts "log-domain: ok"
end
Math.singleton_class.send(:alias_method, :save_log10, :log10)
p Math.save_log10(100)
