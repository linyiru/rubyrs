# is_a?/kind_of? must consult modules `extend`ed into the object's eigenclass
# (extend puts the module in the singleton class's includes; the eigenclass's
# superclass is the real class, so regular ancestry stays covered).
# instance_of? stays STRICT (real class only).
module M; end
module N; end
module Deep; end
module Mid; include Deep; end

o = Object.new
p o.is_a?(M)              # false
o.extend(M)
p o.is_a?(M)             # true
p o.kind_of?(M)          # true
p o.is_a?(N)             # false
p o.instance_of?(Object) # true
p o.is_a?(Object)        # true (regular ancestry still found)

# extend a module that itself includes another → both reachable
x = Object.new
x.extend(Mid)
p x.is_a?(Mid)           # true
p x.is_a?(Deep)          # true (transitive through Mid's include)

# user-class instance + extend
class C; end
c = C.new
c.extend(M)
p c.is_a?(M)             # true
p c.is_a?(C)             # true
p c.instance_of?(C)      # true
