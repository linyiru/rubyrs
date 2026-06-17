# String#scan with a block publishes `$~` (and its derived globals)
# for each successive match, exactly as CRuby does. dotenv's parser
# reads `$LAST_MATCH_INFO[:key]` inside such a block.
require "English"

# Named captures via $~ / $LAST_MATCH_INFO inside the block.
keys = []
vals = []
"A=1\nB=2".scan(/(?<key>\w+)=(?<val>\w+)/) do
  m = $LAST_MATCH_INFO
  keys << m[:key]
  vals << m[:val]
end
p keys
p vals

# Positional captures via $1 / $2 and the whole match via $&.
pairs = []
"x10 y20".scan(/([a-z])(\d+)/) do
  pairs << [$&, $1, $2]
end
p pairs

# No-group pattern still sets $~ to the whole match each iteration.
wholes = []
"aXbXc".scan(/X/) { wholes << $~[0] }
p wholes

# $~ from inside the block is method-scoped: it must not leak the
# block's last match into a caller that never matched.
def runs_scan
  "p=q".scan(/(?<k>\w)=(?<v>\w)/) { |_| }
  $~ && $~[:k]
end
p runs_scan

# pre/post match globals track the active iteration.
prepost = []
"ab12cd34".scan(/\d+/) { prepost << [$`, $'] }
p prepost
