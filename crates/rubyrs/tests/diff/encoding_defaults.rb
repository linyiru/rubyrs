# Encoding.default_external/internal exist and return Encoding
# objects. The VALUE is locale-dependent on CRuby (CI runners
# without LANG report US-ASCII), so only the shape is asserted —
# rubyrs is UTF-8 throughout, documented as equivalent to
# `-Eutf-8`.
p Encoding.default_external.is_a?(Encoding)
p Encoding.respond_to?(:default_external=)
p Encoding.respond_to?(:default_internal)
p Encoding.respond_to?(:default_internal=)
