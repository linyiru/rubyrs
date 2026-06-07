module ScopedOuter
  class Inner
    DEEP = "loaded-deep"
    def self.greet; "hi from inner"; end
  end
end
