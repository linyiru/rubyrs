# rubyrs native Commissioner walk driver hook (commdrv).
#
# Injected by the require handler right after `require "rubocop"` finishes
# (see vm/kernel.rs). Routes `RuboCop::Cop::Commissioner#walk` — the cop
# investigate hot loop — through the native driver (`__rubyrs_commdrv_*`
# host fns): precomputed per-node-type callback tables with resolved method
# handles instead of per-node `public_send(:"on_#{type}")`, and a native
# AST traversal instead of the Traversal recursion trampoline. Falls back
# to the interpreted walk whenever the native driver declines a shape
# (subclassed/patched Commissioner, unknown node types, non-public
# callbacks, ...). RUBYRS_COMMDRV_NO_NATIVE=1 is the kill switch.
#
# Error-handling fidelity: a cop callback raising StandardError must behave
# exactly like Commissioner#with_cop_error_handling. The native driver does
# NOT rescue in Rust — the raise unwinds to the `rescue ::StandardError`
# below (a real interpreted rescue: filter matching, throw-carrier
# transparency, `$!` scoping all come from the VM's stock machinery), the
# recorder mirrors with_cop_error_handling's rescue body, and the loop
# resumes the native walk from the saved position. Non-StandardError
# exceptions and `throw`s unwind past this rescue and abandon the walk,
# exactly as they'd unwind past the interpreted wrapper; the `ensure`
# releases the native state either way.
# The injection keys on the require PATH ("rubocop"), so guard against a
# same-named non-rubocop library, an ancient Commissioner without #walk,
# and a re-injection (a second load of rubocop from another canonical
# path) — re-aliasing :walk after our override is installed would make
# __rubyrs_interp_walk recurse into itself.
if defined?(::RuboCop::Cop::Commissioner) &&
   ::RuboCop::Cop::Commissioner.is_a?(::Class) &&
   ::RuboCop::Cop::Commissioner.method_defined?(:walk) &&
   !::RuboCop::Cop::Commissioner.method_defined?(:__rubyrs_interp_walk)

module RuboCop
  module Cop
    class Commissioner
      COMMDRV_NATIVE =
        begin
          !ENV["RUBYRS_COMMDRV_NO_NATIVE"] &&
            defined?(__rubyrs_commdrv_seal) &&
            defined?(::RuboCop::AST::Traversal) &&
            defined?(::RuboCop::AST::SendNode) &&
            defined?(::RuboCop::AST::Node) &&
            !!__rubyrs_commdrv_seal(::RuboCop::Cop::Commissioner,
                                    ::RuboCop::AST::Traversal,
                                    ::RuboCop::AST::SendNode,
                                    ::RuboCop::AST::Node)
        rescue ::StandardError
          false
        end

      alias_method :__rubyrs_interp_walk, :walk

      def walk(node)
        if COMMDRV_NATIVE && !node.nil?
          # start validates every shape it depends on and flattens the
          # visit order BEFORE any callback fires; nil = decline. `false`
          # means the driver's thread-local seal is missing or from
          # another VM generation (fresh process after a snapshot
          # restore, or a second Runtime on this thread) — reseal against
          # the live constants and retry once. The instance_of? gate
          # keeps Commissioner SUBCLASS walks (also reported as `false`)
          # from paying a per-walk reseal.
          state = __rubyrs_commdrv_start(self, node)
          if false == state && instance_of?(::RuboCop::Cop::Commissioner)
            __rubyrs_commdrv_seal(::RuboCop::Cop::Commissioner,
                                  ::RuboCop::AST::Traversal,
                                  ::RuboCop::AST::SendNode,
                                  ::RuboCop::AST::Node)
            state = __rubyrs_commdrv_start(self, node)
          end
          if state.is_a?(::Integer)
            begin
              done = false
              until done
                begin
                  done = __rubyrs_commdrv_run(state)
                rescue ::StandardError => e
                  __rubyrs_commdrv_cop_error(state, e)
                end
              end
            ensure
              __rubyrs_commdrv_free(state)
            end
            return nil
          end
        end
        __rubyrs_interp_walk(node)
      end

      # The rescue body of with_cop_error_handling, verbatim — the pending
      # (cop, node) is whatever callback invocation the native driver
      # paused on.
      def __rubyrs_commdrv_cop_error(state, e)
        raise e if @options[:raise_error]
        cop, node = __rubyrs_commdrv_pending(state)
        err = ::RuboCop::ErrorWithAnalyzedFileLocation.new(cause: e, node: node, cop: cop)
        raise err if @options[:raise_cop_error]
        @errors << err
        nil
      end
    end
  end
end

end
