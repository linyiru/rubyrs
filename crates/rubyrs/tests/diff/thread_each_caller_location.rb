# Thread.each_caller_location yields the caller's locations (the
# frame that called it excluded) and returns nil; `break` stops it.
def first_caller
  Thread.each_caller_location { |loc| break loc.base_label }
end

def calls_first_caller = first_caller

def walk_all
  n = 0
  r = Thread.each_caller_location { |loc| n += 1 if loc.respond_to?(:path) }
  [r, n.positive?]
end

p calls_first_caller
p walk_all
