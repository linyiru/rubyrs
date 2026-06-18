# A singleton method defined on a heap primitive (Proc/Array/String) is
# dispatched even in the block-call form. Sinatra's result_test sets a
# lambda-with-#each as the Rack body; Rack calls `body.each { ... }`.
res = lambda { 'Hello World' }
def res.each; yield call; end
res.each { |x| p x }

arr = [1, 2]
def arr.each_pair; yield self[0], self[1]; end
arr.each_pair { |a, b| p [a, b] }

s = "hi".dup
def s.shout; yield upcase; end
s.shout { |u| p u }
