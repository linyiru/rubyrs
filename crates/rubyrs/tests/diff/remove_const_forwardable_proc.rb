# Wall-break batch from minitest's own suite: Module#remove_const,
# Forwardable on plain objects + delegate hash form, Proc#===/to_proc.
ALPHA = 1
p Object.send(:remove_const, :ALPHA)
p defined?(ALPHA)
module Holder
  INNER = "x"
end
p Holder.send(:remove_const, :INNER)
p defined?(Holder::INNER)
class KlassConst; end
p Object.send(:remove_const, :KlassConst).inspect
p defined?(KlassConst)
begin
  Object.send(:remove_const, :NOPE_Z)
rescue NameError
  puts "NameError: ok"
end
begin
  Object.send(:remove_const, :lower)
rescue NameError
  puts "wrong-name NameError: ok"
end

require "forwardable"
class FBox
  def initialize; @items = [10, 20]; end
  attr_reader :items
end
fb = FBox.new
fb.extend Forwardable
fb.delegate :first => :items
fb.delegate %i[size last] => :items
p [fb.first, fb.size, fb.last]
class FCBox
  extend Forwardable
  def initialize; @arr = [7, 8]; end
  attr_reader :arr
  def_delegators :arr, :size, :first
end
fc = FCBox.new
p [fc.size, fc.first]

pr = proc { |x| "v-#{x}" }
p(pr === 5)
p pr.to_proc.equal?(pr)
matcher = ->(n) { n > 3 }
case 4
when matcher then puts "case-proc: ok"
else puts "case-proc: MISS"
end
