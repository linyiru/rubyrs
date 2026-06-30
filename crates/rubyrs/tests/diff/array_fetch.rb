p [10, 20, 30].fetch(1)
p [10, 20, 30].fetch(-1)
p [10, 20].fetch(5, :default)
p [10, 20].fetch(5) { |i| "blk#{i}" }
begin; [1, 2].fetch(9); rescue => e; p [e.class.to_s, e.message]; end
begin; [1, 2].fetch(-9); rescue => e; p [e.class.to_s, e.message]; end
