describe :two_method_shared, shared: true do
  it "uses both placeholders correctly" do
    obj.send(@method).should == 1
    obj.send(@method2).should == 2
  end
end
