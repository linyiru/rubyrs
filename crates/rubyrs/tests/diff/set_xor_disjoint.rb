require "set"
# Set#^ (symmetric difference, CRuby insertion order), #disjoint?,
# #intersect?.
p Set[1, 2] ^ Set[2, 3]
p Set[1, 2, 3] ^ Set[]
p Set[] ^ Set[1]
p (Set[1, 2] ^ Set[1, 2]).empty?
p Set[1, 2].disjoint?(Set[3, 4])
p Set[1, 2].disjoint?(Set[2])
p Set[].disjoint?(Set[1])
p Set[1, 2].intersect?(Set[2])
p Set[1].intersect?(Set[2])
p Set[1, 2].respond_to?(:disjoint?)
