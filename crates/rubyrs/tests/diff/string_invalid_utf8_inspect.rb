# Invalid-UTF-8 byte runs render as \xNN in inspect (valid runs
# keep normal char escapes) — minitest's mu_pp encoding headers
# compare these shapes for bad-encoding fixtures.
p "\xB6"
p "\xB6A\nB"
p "ok\xFF!"
p "héllo"
p ["\xB6"].inspect
p "\xB6".valid_encoding?
p "ok".valid_encoding?
