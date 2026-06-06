# `:"#{expr}…"` — interpolated symbol (string interpolation → Symbol).
# Discovery: P3 Jekyll spike — jekyll builds setter symbols like
# `:"#{key}="` dynamically.
k = "title"
p :"#{k}="
p :"a#{1 + 1}b"
p :"#{k}".class
p :"prefix_#{k}_suffix"
n = 3
p [:"item#{n}", :"item#{n + 1}"]
p :"#{k}=".to_s
