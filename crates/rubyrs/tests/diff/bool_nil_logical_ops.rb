# Boolean / NilClass logical METHODS & | ^ — non-short-circuiting,
# truthiness-based, any object argument.
p true & false
p true & nil
p true & 1
p true & "x"
p false & true
p true | false
p false | nil
p false | 5
p true ^ true
p true ^ nil
p false ^ "x"
p nil & true
p nil | false
p nil | 7
p nil ^ nil
p nil ^ 3
p true.respond_to?(:&)
p nil.respond_to?(:|)
