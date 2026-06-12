# GC rooting for ANONYMOUS classes reachable only through heap
# containers: their define_method closure captures, class-level
# ivars, class_vars, and per-class consts must survive collection.
# minitest's describe-generated Spec subclasses (held only by the
# @@runnables registry) hit every one of these — a captured
# `before`-block was swept and instance_exec'd post-free.
# Meaningful under STRESS_GC=1 (the CI stress job); benign otherwise.
registry = []
10.times do |i|
  payload = "payload-#{i}"
  c = Class.new do
    define_method(:fetch) { payload * 2 }
  end
  c.instance_variable_set(:@meta, ["meta-#{i}"])
  registry << c
end
# allocation churn so a stressed GC has plenty of chances to sweep
500.times { |i| [i.to_s * 3, { i => [i] }] }
registry.each_with_index do |c, i|
  raise "closure lost at #{i}" unless c.new.fetch == "payload-#{i}payload-#{i}"
  raise "ivar lost at #{i}" unless c.instance_variable_get(:@meta) == ["meta-#{i}"]
end
puts "anon-class GC roots OK"
# named-class ivar registry (the Vm.classes walk side)
class NamedRegistry
  @store = []
  class << self
    attr_reader :store
  end
end
100.times { |i| NamedRegistry.store << "s#{i}" if i % 25 == 0 }
200.times { |i| [i.to_s] }
p NamedRegistry.store.length
puts "named-class ivar roots OK"
