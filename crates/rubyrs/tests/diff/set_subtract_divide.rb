require "set"
# subtract / keep_if / select! / reject! (in-place), classify, divide
# (the 1-arity "group by block value" form).
p Set[1, 2, 3].subtract([1]).to_a.sort
p Set[1, 2, 3, 4].keep_if { |x| x.even? }.to_a.sort
p Set[1, 2, 3].select! { |x| x > 1 }.to_a.sort
p Set[1, 2, 3].select! { |x| x > 0 }.inspect
p Set[1, 2, 3].reject! { |x| x.even? }.to_a.sort
p Set[1, 3, 5].reject! { |x| x.even? }.inspect
p Set[1, 2, 3, 4].classify { |x| x.even? }.transform_values { |v| v.to_a.sort }
p Set[1, 2, 3, 4].divide { |x| x.even? }.map { |s| s.to_a.sort }.sort
p Set[1, 2, 3].subtract(Set[2]).to_a.sort
