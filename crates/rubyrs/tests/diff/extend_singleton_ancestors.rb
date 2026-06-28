# `extend M` reflects M in the singleton class's ancestors / included_modules /
# include?, consistent with method dispatch (previously only dispatch saw the
# extended module). ActiveSupport::Concern's dependency logic + bare-const
# resolution walk these, so they must match CRuby.
module M
  def hello; "hi from M"; end
end
class C; end
C.extend(M)
sc = C.singleton_class
p sc.ancestors.include?(M)
p sc.include?(M)
p sc.included_modules.include?(M)
p C.hello
