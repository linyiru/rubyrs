# A bare method call in a `class << self` body runs with self = the singleton
# class, so a def-generating macro (ActiveSupport's `delegate`, which
# module_evals `def`s) installs CLASS methods — ActiveRecord::ExplainRegistry's
# `class << self; delegate :collect?, to: :instance; end` shape.
class Inner
  def bar?; "barq"; end
  def plain; "plain"; end
end
module Delegator
  def delegate(*names, to:)
    names.each do |n|
      module_eval("def #{n}(*a, &b); #{to}.#{n}(*a, &b); end")
    end
  end
end
class Foo
  def self.inst; @inst ||= Inner.new; end
  class << self
    extend Delegator
    delegate :bar?, :plain, to: :inst
  end
end
p Foo.plain
p Foo.bar?
