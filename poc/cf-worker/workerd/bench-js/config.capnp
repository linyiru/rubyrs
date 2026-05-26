# JS-only baseline — measures workerd process startup + V8
# isolate spawn cost. Used to attribute the rubyrs worker's
# cold-start time to wasm/Ruby overhead vs workerd's own
# inherent setup.
using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  services = [ ( name = "main", worker = .helloWorker ) ],
  sockets  = [ ( name = "http", address = "*:8081", http = (), service = "main" ) ]
);

const helloWorker :Workerd.Worker = (
  modules = [
    ( name = "hello.mjs", esModule = embed "./hello.mjs" ),
  ],
  compatibilityDate = "2025-11-01",
);
