# Divergence ratchet: String#strip / #lstrip / #rstrip do not strip
# trailing/leading NUL bytes in rubyrs.
#
# CRuby's strip family removes ASCII whitespace AND zero-bytes from
# the ends of the string. rubyrs only removes ASCII whitespace,
# leaving zero-bytes in place.
#
# When fixed in vm/string.rs (extend the strip predicate to include
# `\x00`), regen this fixture via UPDATE_EXPECTED=1 AND un-skip the
# three `# skipped (divergent):` traces in:
#   - spec/ruby/string_strip_spec.rb  (NULL bytes + whitespace block)
#   - spec/ruby/string_lstrip_spec.rb (strips leading \\0 block)
#   - spec/ruby/string_rstrip_spec.rb (trailing whitespace and NULL bytes block)

# All three strip variants leave the NUL bytes intact.
puts "strip:  #{"\x00 hello \x00".strip.inspect}"
puts "lstrip: #{"\x00 hello".lstrip.inspect}"
puts "rstrip: #{"hello \x00".rstrip.inspect}"

# Plain whitespace still strips correctly (only the NUL path diverges).
puts "strip:  #{"  hello  ".strip.inspect}"
puts "lstrip: #{"  hello".lstrip.inspect}"
puts "rstrip: #{"hello  ".rstrip.inspect}"
