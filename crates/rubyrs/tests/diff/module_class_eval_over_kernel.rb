# A user `Kernel#class_eval` (ActiveSupport's
# core_ext/kernel/singleton_class.rb defines exactly this) must not
# shadow `Module#class_eval` for a Class receiver: Module precedes
# Kernel in a class's ancestry. Non-module receivers DO reach the
# Kernel method, block and string forms alike.
module Kernel
  def class_eval(*args, &block)
    singleton_class.class_eval(*args, &block)
  end
end

class Host
  # Bare call, self = the class (class_attribute's reader codegen).
  def self.define_reader
    class_eval("def reader; :instance; end")
  end

  def self.define_block_reader
    class_eval { def block_reader; :instance_blk; end }
  end
end

Host.define_reader
Host.define_block_reader
p Host.new.reader
p Host.new.block_reader
p Host.method_defined?(:reader)
p Host.singleton_class.method_defined?(:reader)

o = Object.new
o.class_eval("def only_me; :singleton; end")
o.class_eval { def only_me_blk; :singleton_blk; end }
p o.only_me
p o.only_me_blk
p Object.new.respond_to?(:only_me)
p Object.new.respond_to?(:only_me_blk)
