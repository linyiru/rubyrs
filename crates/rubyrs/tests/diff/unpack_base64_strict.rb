# String#unpack("m0") — STRICT Base64 (RFC 4648), distinct from the
# tolerant "m" (RFC 2045). m0 rejects non-multiple-of-4 length,
# whitespace / non-alphabet bytes, misplaced padding, and non-canonical
# leftover bits with ArgumentError "invalid base64". The base64 stdlib
# gem's strict_decode64 / urlsafe_decode64 (and thus jwt) rely on it.
["", "YWJj", "YW==", "YWI=", "abc", "YWJjx", "YWJjeA==", "YW J j",
 "YWJj\n", "Y", "====", "QQ==", "YQ=="].each do |s|
  begin
    p [s, s.unpack1("m0")]
  rescue => e
    puts "#{s.inspect} ERR #{e.class}: #{e.message}"
  end
end

# Tolerant "m" still skips whitespace and stops at padding.
p "YW J j".unpack1("m")
p "YWJj\n".unpack1("m")
p "abc".unpack1("m")
