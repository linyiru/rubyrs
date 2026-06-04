# Minimal `require 'ipaddr'` stub. HostAuthorization's
# constructor pattern-matches each permitted-hosts entry on
# `case host; when String; ...; when IPAddr; ...; end`. The
# `when IPAddr` arm only fires for IPAddr instances; the
# fixture passes only String hosts so the arm never runs, but
# the constant must exist for the `case` to dispatch without
# a NameError on class load.
#
# Real CRuby's `ipaddr` stdlib ships a full IPAddr class with
# parsing, range / membership ops, and an InvalidAddressError
# error class. The middleware's `rescue
# IPAddr::InvalidAddressError` clause needs that nested class
# to resolve at parse time.
class IPAddr
  class InvalidAddressError < StandardError; end
end
