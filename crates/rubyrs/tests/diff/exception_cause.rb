# Exception#cause exists and returns nil for an exception raised without
# an active cause chain (CRuby parity for the raised-directly case).
# minitest's assert_raises calls #cause on the caught exception.
begin
  raise ArgumentError, "boom"
rescue => e
  p e.cause                 # nil
  p e.respond_to?(:cause)   # true
end

class MyErr < StandardError; end
begin
  raise MyErr, "x"
rescue MyErr => e
  p e.cause                 # nil
  p e.message               # "x"
end
