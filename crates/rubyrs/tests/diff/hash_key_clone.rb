# Hash#key (reverse lookup), Hash#clone (shallow copy preserving the
# subclass tag), and the Hash[] / subclass[] constructor. Discovery:
# P3 Jekyll spike — jekyll's Configuration < Hash uses
# Configuration[override], log_adapter uses LOG_LEVELS.key, and
# read_config_files uses `clone`.
h = {a: 1, b: 2, c: 2}
p h.key(2)            # first key whose value == 2
p h.key(99)           # nil when absent
p h.key(1)

c = h.clone
p c == h
p c.equal?(h)         # distinct object
c[:d] = 4
p h.key?(:d)          # clone is independent

# Hash[] constructor (all shapes)
p Hash[[[:x, 1], [:y, 2]]]
p Hash[:a, 1, :b, 2]
p Hash[{m: 9}]

# Subclass: Hash[] builds the subclass; clone/dup stay the subclass
class Conf < Hash; end
cf = Conf[{k: 1}]
p cf.class
p cf[:k]
p cf.merge!({j: 2})   # inherited primitive
p cf.clone.class      # clone preserves the subclass
p cf.dup.class
