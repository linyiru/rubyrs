# CRuby nests Mutex / ConditionVariable / Queue under Thread, the same
# object as the top-level constant. And Thread.handle_interrupt masks
# async interrupts (a no-op that runs the block in the single-thread
# model). connection_pool's TimedStack + #with use both.
p Thread::Mutex.equal?(Mutex)                       # true
p Thread::ConditionVariable.equal?(ConditionVariable) # true
p Thread::Queue.equal?(Queue)                       # true

m = Thread::Mutex.new
p m.synchronize { 1 + 1 }                           # 2
cv = Thread::ConditionVariable.new
p cv.is_a?(ConditionVariable)                       # true
# (cv.class.name differs: CRuby's canonical name is the nested
# Thread::ConditionVariable; rubyrs's is the top-level alias — a benign
# which-is-canonical divergence, so assert identity not the printed name.)

# handle_interrupt runs the block and returns its value
r = Thread.handle_interrupt(Exception => :never) { 40 + 2 }
p r                                                 # 42
p Thread.pending_interrupt?                         # false
