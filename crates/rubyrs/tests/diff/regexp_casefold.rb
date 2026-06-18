p(/foo/i.casefold?)
p(/foo/.casefold?)
p(/foo/m.casefold?)
p(/foo/mix.casefold?)
p Regexp.new("a", Regexp::IGNORECASE).casefold?
p Regexp.new("a").casefold?
