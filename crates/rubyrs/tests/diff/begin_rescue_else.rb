def t1
  begin; y = 1; rescue => e; return "rescue"; else; return "else-ran"; end
  "fell-through"
end
p t1

def t2
  out = ""
  begin; out << "body"; rescue => e; out << "rescue"; else; out << "-else"; end
  out
end
p t2

def t3
  begin; raise "boom"; rescue => e; "caught:#{e.message}"; else; "else-skipped"; end
end
p t3

# value of begin is else's value
v = begin; 10; rescue; 20; else; 30; end
p v

# nested begin/else flag collision
def t4
  begin
    begin; a = 1; rescue; "ir"; else; b = "inner-else"; end
  rescue; "or"
  else; "outer-else"
  end
end
p t4

# else exception NOT caught by rescue
def t5
  begin
    1
  rescue RuntimeError
    "wrong-caught"
  else
    raise "from-else"
  end
rescue => e
  "outer:#{e.message}"
end
p t5

# ensure runs after else
def t6
  log = []
  begin
    log << :body
  rescue
    log << :rescue
  else
    log << :else
  ensure
    log << :ensure
  end
  log
end
p t6
