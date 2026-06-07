# select! / reject! / keep_if / delete_if retain the pre-break deletions
# AND every un-visited element when the block does `break` or a non-local
# `return`. Regression: the early return discarded the filtered prefix
# entirely, leaving the receiver array unchanged.
b = [1, 2, 3, 4]; r = b.select! { |x| break :s if x == 3; x.odd? }; p [b, r]
b = [1, 2, 3, 4]; r = b.reject! { |x| break :s if x == 3; x.odd? }; p [b, r]
b = [1, 2, 3, 4, 5]; r = b.keep_if { |x| break :s if x == 4; x.odd? }; p [b, r]
b = [1, 2, 3, 4, 5]; r = b.delete_if { |x| break :s if x == 4; x.odd? }; p [b, r]

def filter_with_return(b)
  b.select! { |x| return :ret if x == 3; x.odd? }
end
b = [1, 2, 3, 4]; r = filter_with_return(b); p [b, r]

# No break taken, nothing changed → select! returns nil.
b = [1, 3, 5]; r = b.select! { |x| break :s if x == 2; x.odd? }; p [b, r]
