# A user subclass of Hash is a real Hash: it reports the subclass as
# its class, is_a?(Hash), and inherits every Hash primitive (`[]=`,
# `[]`, `merge!`, `size`, `keys`, `each`, `to_h`, …). Discovery: P3
# Jekyll spike — safe_yaml's `CaseAgnosticMap < Hash`; also unblocks
# Rack::Headers / ActiveSupport HashWithIndifferentAccess.
class M < Hash
end

m = M.new
p m.class
p m.is_a?(Hash)
p m.is_a?(M)
p m.instance_of?(M)
p m.instance_of?(Hash)
p m.kind_of?(Hash)

m[:a] = 1
m[:b] = 2
p m[:a]
p m.size
p m.length
p m.empty?
p m.key?(:a)
p m.keys
p m.values
p m.merge!({c: 3})
p m.to_h
p m.map { |k, v| [k, v * 10] }
collected = []
m.each { |k, v| collected << "#{k}=#{v}" }
p collected
p m.select { |k, v| v > 1 }
p m.fetch(:a)
p m.fetch(:zzz, :default)

# subclass instances are independent
n = M.new
p n.size
p m.size
