# Errno::EALREADY and Errno::ECONNABORTED — the two socket Errno classes
# rubyrs was missing from faraday-net_http's exception list (net_http.rb:18).
# Both are SystemCallError subclasses (caught by a bare `rescue`).
p Errno::EALREADY < SystemCallError
p Errno::ECONNABORTED < SystemCallError
p Errno::EALREADY < StandardError

begin
  raise Errno::ECONNABORTED
rescue SystemCallError => e
  p e.class.name
end

begin
  raise Errno::EALREADY
rescue => e            # bare rescue catches StandardError descendants
  p e.class.name
end

# The full faraday-net_http Errno set now resolves.
%i[EADDRNOTAVAIL EALREADY ECONNABORTED ECONNREFUSED ECONNRESET
   EHOSTUNREACH EINVAL ENETUNREACH EPIPE].each do |n|
  p [n, Errno.const_get(n) < SystemCallError]
end
