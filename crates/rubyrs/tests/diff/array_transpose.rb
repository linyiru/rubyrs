# Array#transpose — rows↔columns; equal-length rows required.
p [[1, 2], [3, 4]].transpose
p [[1, 2, 3], [4, 5, 6]].transpose
p [[1], [2], [3]].transpose
p [].transpose
p [["a", "b"], ["c", "d"]].transpose
begin; [[1, 2], [3]].transpose; rescue => e; p [e.class, e.message]; end
begin; [[1, 2], 3].transpose; rescue => e; p e.class; end
p [[1, 2], [3, 4]].respond_to?(:transpose)
