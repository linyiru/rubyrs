# Matching a BINARY (ASCII-8BIT) subject against an `/n` regex is
# byte-level (CRuby): `/[\x80-\xff]/n` matches a raw high byte, not a
# decoded codepoint. rack's Lint checks CGI env values with
# `value.b !~ /[\x80-\xff]/n`. (UTF-8 subjects are unaffected.)

bin = "ሴ".b                      # bytes [0xE1,0x88,0xB4], tagged BINARY
p (bin =~ /[\x80-\xff]/n)        # 0  (first byte 0xE1 is in range)
p (bin !~ /[\x80-\xff]/n)        # false
p bin.match?(/[\x80-\xff]/n)     # true
p ("A\xE1".b =~ /[\x80-\xff]/n)  # 1  (byte index of the high byte)
p ("ABC".b =~ /[\x80-\xff]/n)    # nil (all low bytes)
p ("ABC".b !~ /[\x80-\xff]/n)    # true
p ("\xff".b.match?(/[\x80-\xff]/n))   # true
p ("\x7f".b.match?(/[\x80-\xff]/n))   # false (0x7f below range)

# a single-byte escape on a binary subject
p ("X\xE1Y".b =~ /\xE1/n)        # 1
p ("XYZ".b =~ /\xE1/n)           # nil

# ASCII byte ranges still work on binary subjects
p ("hello".b =~ /[\x61-\x7a]/n)  # 0

# UTF-8 subject is UNCHANGED — codepoint semantics, not bytes
p ("café" =~ /\w+/)              # 0
p ("café".match?(/é/))           # true
p ("hello world".scan(/\w+/))    # ["hello", "world"]
p ("a1b2".gsub(/\d/, "#"))       # "a#b#"
