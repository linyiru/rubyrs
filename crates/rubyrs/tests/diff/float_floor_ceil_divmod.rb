# Float#floor(ndigits) / #ceil(ndigits): n>0 → Float at that place
# (toward -∞/+∞), n<=0 → Integer. Float#divmod → [Integer q, Float r].
p 12345.6789.floor(2)
p 12345.6789.ceil(-2)
p 12345.6789.floor(-2)
p 12345.6789.ceil(2)
p 2.345.floor(2)
p 2.341.ceil(2)
p 3.7.floor(0)
p 3.2.ceil(0)
p (-2.5).floor(0)
p (-2.5).ceil(0)
p 3.7.floor          # no-arg still works
p 3.2.ceil
# divmod
p 7.0.divmod(3)
p (-7.0).divmod(3)
p 7.0.divmod(2.5)
p 7.5.divmod(2.5)
p 0.0.divmod(3)
p 7.0.respond_to?(:divmod)
begin; 7.0.divmod(0); rescue => e; p e.class; end
begin; 7.0.divmod(0.0); rescue => e; p e.class; end
begin; (1.0/0).divmod(3); rescue => e; p e.class; end
begin; 7.0.divmod(Float::NAN); rescue => e; p e.class; end
begin; (1.0/0).floor(2); rescue => e; p e.class; end
