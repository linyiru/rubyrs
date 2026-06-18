# Regexp#named_captures returns name => ALL its 1-based group indices,
# in first source-appearance order (the engines collapse duplicate
# (?<a>…) names). mustermann maps multiple `*` splat captures this way.
p(/(?<splat>[^\/]+)\/(?<splat>[^\/]+)\/(?<splat>.*)/.named_captures)
p(/(?<a>.)(?<b>.)(?<a>.)/.named_captures)
p(/(?<x>.)(?<y>.)/.named_captures)
p(/(?<dup>.)(?<dup>.)/.named_captures)
