# A frozen String's mutation FrozenError renders the receiver's
# INSPECT (`can't modify frozen String: "y"`), not its raw bytes —
# CRuby parity. Surfaced widely once `# frozen_string_literal: true`
# made frozen literals common. The inspect is encoding-aware (escapes,
# BINARY \xNN), shared with String#inspect via `rstr_inspect`.

[
  "y",
  "café",
  "tab\there",
  "nl\nhere",
  "quote\"inside",
  "\xFF\xFE".b,        # BINARY — \xNN escapes
].each do |s|
  # exercise a few different mutators — all share the message.
  [:<<, :concat, :upcase!, :reverse!].each do |m|
    f = s.dup.freeze
    begin
      m == :<< || m == :concat ? f.send(m, "z") : f.send(m)
      puts "#{m}: no raise"
    rescue FrozenError => e
      puts "#{m}: #{e.message}"
    end
  end
end

# The message matches String#inspect exactly.
fr = "a\tb".freeze
msg = (fr << "x" rescue $!.message)
puts msg
puts "matches_inspect=#{msg == "can't modify frozen String: #{"a\tb".inspect}"}"
