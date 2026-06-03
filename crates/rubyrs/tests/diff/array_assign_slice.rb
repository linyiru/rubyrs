# Array#[]= splice assignment — two-arg `start, length` form and
# Range form. Companion to `array_subscript_slice.rb` (which
# covers Array#[] read forms); together they close the
# subscript-pair surface so user code can write the natural
# Ruby for slice mutation.

# --- Single-Integer write (pre-existing, regression-guard) ---
a = [1, 2, 3]; a[1] = 99; p a
a = [1, 2, 3]; a[5] = 7;  p a       # pads with nil
a = [1, 2, 3]; a[-1] = 0; p a       # negative index

# --- Two-arg `start, length = value` ---
a = [1, 2, 3, 4, 5]; a[1, 2] = [9, 8];           p a
a = [1, 2, 3, 4, 5]; a[1, 2] = 99;               p a     # non-Array RHS → single-element replace
a = [1, 2, 3, 4, 5]; a[1, 0] = [9];              p a     # zero-length → pure insert
a = [1, 2, 3, 4, 5]; a[1, 2] = [];               p a     # empty Array → pure delete
a = [1, 2, 3, 4, 5]; a[7, 0] = [9];              p a     # start past len → nil-pad then insert
a = [1, 2, 3, 4, 5]; a[-2, 2] = [9];             p a     # negative start wraps
a = [1, 2, 3, 4, 5]; a[0, 100] = [9];            p a     # length over → clamp at len
a = [1, 2, 3, 4, 5]; a[5, 0] = [9];              p a     # boundary append
a = [1, 2, 3, 4, 5]; a[0, 5] = [];               p a     # delete everything
a = [1, 2, 3, 4, 5]; a[2, 1] = [9, 8, 7];        p a     # expand: replace 1 elem with 3

# --- Error shapes: negative length is IndexError (NOT nil
# like the read form), excessively-negative start with no
# wrap-target is IndexError. ---
begin
  a = [1, 2, 3, 4, 5]; a[1, -1] = [9]
rescue IndexError => e
  puts "IndexError: #{e.message}"
end
begin
  a = [1, 2, 3, 4, 5]; a[-99, 2] = [9]
rescue IndexError => e
  puts "IndexError: #{e.message}"
end

# --- Range form ---
a = [1, 2, 3, 4, 5]; a[1..2] = [9, 8];     p a
a = [1, 2, 3, 4, 5]; a[1...3] = [9, 8];    p a
a = [1, 2, 3, 4, 5]; a[1..-1] = [9];       p a       # negative end
a = [1, 2, 3, 4, 5]; a[1..2] = 99;         p a       # non-Array RHS
a = [1, 2, 3, 4, 5]; a[2..] = [9];         p a       # endless
a = [1, 2, 3, 4, 5]; a[..1] = [9];         p a       # beginless
a = [1, 2, 3, 4, 5]; a[5..5] = [9];        p a       # boundary append
a = [1, 2, 3, 4, 5]; a[6..7] = [9];        p a       # start past len → nil-pad
a = [1, 2, 3, 4, 5]; a[1..0] = [9, 9];     p a       # begin > end → insert without removing
a = [1, 2, 3, 4, 5]; a[0..4] = [];         p a       # delete everything via range

# --- Idiom from real code: insert at the start ---
a = [2, 3, 4]; a[0, 0] = [1]; p a
a = [2, 3, 4]; a[0..-1] = [1, 2]; p a

# --- Idiom: replace a chunk in the middle ---
a = ["hdr", "<a>", "<b>", "<c>", "ftr"]; a[1..3] = ["X", "Y"]; p a
