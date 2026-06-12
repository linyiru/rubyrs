# `defined?(bare_name)` inside a class/module BODY resolves through
# the class-object chain (Class/Module reopens) — minitest's mock.rb
# guards its must_verify infect with it. Proc identity equality
# rides along (matcher tables hold procs).
class Module
  def made_up_helper(*); :mh; end
end
module ProbeMod
  p defined?(made_up_helper)
  p defined?(no_such_helper)
end
class ProbeCls
  p defined?(made_up_helper)
end
pr = proc { 1 }
p [pr, //].include?(pr)
p (pr == pr)
p (pr == proc { 1 })
