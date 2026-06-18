# NoMethodError < NameError < StandardError (CRuby hierarchy). Tilt's
# specs assert_raises(NameError) and expect a NoMethodError to satisfy it.
p NoMethodError.superclass
p NameError.superclass
p(NoMethodError < NameError)
p NoMethodError.ancestors.include?(NameError)
begin; nil.no_such_method; rescue NameError => e; p [:caught, e.class]; end
begin; SomeUndefinedConstant; rescue NameError => e; p e.class; end
