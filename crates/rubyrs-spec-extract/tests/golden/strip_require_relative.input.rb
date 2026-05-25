require_relative '../../spec_helper'
require_relative 'fixtures/classes'
require_relative('../shared/string/length')

# Comment lines about `require_relative` stay — only the
# actual call form gets stripped.
describe "stripping" do
  it "removes the loader lines but keeps the spec body" do
    "hello".length.should == 5
    [1, 2, 3].length.should == 3
  end
end

  require_relative 'indented_one'   # leading whitespace ok
