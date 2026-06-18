# `class << self; prepend M; end` registers M as a singleton prepend;
# `singleton_class.ancestors.first` must return M (the same module the
# dispatch chain uses), so removing a method from it restores dispatch.
# Tilt's finalize!/teardown does exactly this.
module T
  def self.reg; "real"; end
  def self.finalize!
    class << self
      prepend(Module.new do
        def reg(*); raise "no reg after finalize"; end
      end)
    end
  end
end
p T.reg
T.finalize!
r = (begin; T.reg; rescue => e; e.message; end)
p r
mod = T.singleton_class.ancestors.first
p mod.instance_of?(Module)
mod.send(:remove_method, :reg)
p T.reg
