# Variant of break_through_ensure.rb: the script wraps the
# offending loop in an outer `begin / rescue => e` to pin the
# non-rescuable behavior. If we ever route the defensive trap
# through a rescuable variant (e.g. RuntimeError), this script
# would silently catch and exit 0, masking the limitation from
# users. Keeping the trap Uncaught ensures it propagates past
# every script-level rescue and surfaces clearly.
ran = false
begin
  while true
    begin
      break 42
    ensure
      ran = true
    end
  end
rescue => e
  puts "outer caught: #{e.message}"
end
puts "ran=#{ran} (if you see this line, the outer rescue silently swallowed the limitation)"
