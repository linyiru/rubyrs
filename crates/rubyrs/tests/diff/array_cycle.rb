# Array#cycle: block form repeats elements (n times, or forever until
# break); no-block form returns an Enumerator that first/take can drive.
p [1,2,3].cycle.first(7)
p [1,2,3].cycle.take(4)
p [1,2].cycle(2).to_a
r = []; [1,2,3].cycle(2) { |x| r << x }; p r
r2 = []; [1,2,3].cycle { |x| r2 << x; break if r2.length >= 5 }; p r2
p([].cycle { |x| })          # nil (empty)
p [].cycle(3) { |x| }        # nil
p [1,2,3].cycle(0) { |x| }   # nil
p [1,2,3].cycle(-1) { |x| }  # nil
p [].respond_to?(:cycle)
p [1,2,3].cycle(2) { |x| break x*10 if x == 2 }   # break value
