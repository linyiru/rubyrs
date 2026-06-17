# `redo` — re-run the current loop iteration / block body without
# re-checking the condition or advancing the iterator. A core keyword
# rubyrs previously couldn't even compile (the whole file failed).
# Surfaced by rss's rss.rb:1222 (`loop do … redo … end`).

# block redo (loop do)
count = 0; tries = 0
loop do
  tries += 1
  count += 1
  redo if count == 2 && tries < 4
  break if count >= 3
end
p [count, tries]                 # [3, 3]

# while redo — body re-runs, condition not re-checked
n = 0; log = []; i = 0
while i < 3
  i += 1
  log << i
  if i == 2 && n < 1
    n += 1
    redo
  end
end
p log                            # [1, 2, 3]

# until redo
m = 0; ulog = []; j = 0
until j >= 2
  j += 1
  ulog << j
  if j == 1 && m < 1
    m += 1
    redo
  end
end
p ulog                           # [1, 1, 2]

# each-block redo (re-yields the same element)
seen = []; fixed = false
[10, 20].each do |x|
  seen << x
  if x == 20 && !fixed
    fixed = true
    redo
  end
end
p seen                           # [10, 20, 20]

# nested: redo binds to the innermost loop (the while), not the block
outer = []
[1].each do
  k = 0
  while k < 2
    k += 1
    outer << k
    redo if k == 1 && outer.size < 3
  end
end
p outer                          # [1, 1, 1, 2]
