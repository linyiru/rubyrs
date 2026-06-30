array = [1, 2].freeze
splat_array = *array
p splat_array
p splat_array.frozen?
splat_array.pop
p splat_array
p array

class SplatToA
  def to_a
    [3, 4].freeze
  end
end

object = SplatToA.new
splat_object = *object
p splat_object
p splat_object.frozen?
splat_object.pop
p splat_object
