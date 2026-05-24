//! IaC (Infrastructure as Code) PoC.
//!
//! Demonstrates two powerful patterns for using rubyrs as an IaC engine:
//! 1. **Active Builder Style (`yield`)**: The Ruby script executes active code that
//!    defines classes, yields builders to blocks, and registers resources
//!    dynamically via host functions.
//! 2. **Declarative Hash-Return Style**: The Ruby script acts as pure declaration,
//!    returning a nested Hash/Array data structure representing the topology. The
//!    host resolves and unpacks the returned structure using the new host APIs.
//!
//! Run with: `cargo run --release -p rubyrs --example iac_poc`

use std::cell::RefCell;
use std::rc::Rc;

use rubyrs::{Runtime, Value};

// ---------- Host Data Structures ----------

#[derive(Debug, Default)]
struct ServerConfig {
    name: String,
    instance_type: String,
    port: i64,
}

#[derive(Debug, Default)]
struct LoadBalancerConfig {
    name: String,
    routes: Vec<(String, String)>, // (path, target_server)
}

#[derive(Debug, Default)]
struct InfraTopology {
    servers: Vec<ServerConfig>,
    load_balancers: Vec<LoadBalancerConfig>,
}

fn main() {
    println!("--- Running Dual-Style IaC PoC ---\n");

    // =========================================================================
    // Part 1: Active Builder Style using `yield` and Blocks
    // =========================================================================
    run_active_builder_style();

    // =========================================================================
    // Part 2: Declarative Hash-Return Style with Nested Unpacking
    // =========================================================================
    run_declarative_hash_style();
}

// -----------------------------------------------------------------------------
// Part 1: Active Builder Style Implementation
// -----------------------------------------------------------------------------

fn run_active_builder_style() {
    let mut rt = Runtime::new();

    // Shared topology state on the host
    let topology = Rc::new(RefCell::new(InfraTopology::default()));

    // Register host functions to allow the active builders to report resources
    let topo_for_server = topology.clone();
    rt.register_fn("host_register_server", move |args| {
        if let [Value::Str(name), Value::Str(itype), Value::Int(port)] = args {
            topo_for_server.borrow_mut().servers.push(ServerConfig {
                name: name.to_string(),
                instance_type: itype.to_string(),
                port: *port,
            });
        }
        Ok(Value::Nil)
    });

    let topo_for_lb = topology.clone();
    rt.register_fn("host_register_lb", move |args| {
        if let [Value::Str(name)] = args {
            topo_for_lb.borrow_mut().load_balancers.push(LoadBalancerConfig {
                name: name.to_string(),
                routes: Vec::new(),
            });
        }
        Ok(Value::Nil)
    });

    let topo_for_route = topology.clone();
    rt.register_fn("host_register_route", move |args| {
        if let [Value::Str(lb_name), Value::Str(path), Value::Str(target)] = args {
            let mut topo = topo_for_route.borrow_mut();
            if let Some(lb) = topo.load_balancers.iter_mut().find(|l| l.name == **lb_name) {
                lb.routes.push((path.to_string(), target.to_string()));
            }
        }
        Ok(Value::Nil)
    });

    // Preamble defining our DSL classes and method helpers in Ruby
    let preamble = r#"
        class ServerBuilder
          def initialize(name)
            @name = name
            @type = "t3.micro"
            @port = 80
          end
          def type=(t); @type = t; end
          def port=(p); @port = p; end
          def build!
            host_register_server(@name, @type, @port)
          end
        end

        class LbBuilder
          def initialize(name)
            @name = name
          end
          def route(path, to)
            host_register_route(@name, path, to)
          end
          def build!
            host_register_lb(@name)
          end
        end

        def server(name)
          builder = ServerBuilder.new(name)
          yield builder
          builder.build!
        end

        def load_balancer(name)
          builder = LbBuilder.new(name)
          builder.build! # registers the LB first
          yield builder
        end
    "#;

    rt.eval(preamble, "dsl_preamble.rb").unwrap();

    // The user's configuration script using our elegant block/builder DSL
    let config_script = r#"
        server "web-1" do |s|
          s.type = "t3.medium"
          s.port = 80
        end

        server "web-2" do |s|
          s.type = "t3.medium"
          s.port = 80
        end

        server "db-primary" do |s|
          s.type = "db.r6g.large"
          s.port = 5432
        end

        load_balancer "public-lb" do |lb|
          lb.route "/", "web-1"
          lb.route "/api", "web-2"
        end
    "#;

    rt.eval(config_script, "user_config.rb").unwrap();

    // Print the topology that was populated dynamically on the host!
    print_topology("Active Builder Style (with yield)", &topology.borrow());
}

// -----------------------------------------------------------------------------
// Part 2: Declarative Hash-Return Style Implementation
// -----------------------------------------------------------------------------

fn run_declarative_hash_style() {
    let mut rt = Runtime::new();

    // The user's configuration script, simply returning a clean declarative Hash
    let config_script = r#"
        {
          servers: [
            { name: "web-1", type: "t3.medium", port: 80 },
            { name: "web-2", type: "t3.medium", port: 80 },
            { name: "db-primary", type: "db.r6g.large", port: 5432 }
          ],
          load_balancers: [
            {
              name: "public-lb",
              routes: [
                { path: "/", target: "web-1" },
                { path: "/api", target: "web-2" }
              ]
            }
          ]
        }
    "#;

    // Evaluate the script and get the returned Hash Value
    let returned_value = rt.eval(config_script, "declarative_config.rb").unwrap();

    // Parse the returned nested structures on the host using the new inspect APIs!
    let topology = parse_declarative_topology(&rt, &returned_value)
        .expect("Failed to parse returned configuration Hash");

    print_topology("Declarative Hash-Return Style", &topology);
}

/// Helper function using Runtime's resolve APIs to walk the returned Hash and Array structures.
fn parse_declarative_topology(rt: &Runtime, val: &Value) -> Option<InfraTopology> {
    let mut topology = InfraTopology::default();

    // Unpack the top-level Hash
    let entries = rt.resolve_hash(val)?;
    for (k, v) in entries {
        if let Value::Sym(sym_id) = k {
            let key_str = rt.resolve_sym(sym_id);
            if key_str == "servers" {
                // Parse servers Array
                let s_array = rt.resolve_array(&v)?;
                for s_val in s_array {
                    let s_hash = rt.resolve_hash(&s_val)?;
                    let mut server = ServerConfig::default();
                    for (sk, sv) in s_hash {
                        if let Value::Sym(ssym) = sk {
                            match rt.resolve_sym(ssym) {
                                "name" => if let Value::Str(s) = sv { server.name = s.to_string(); },
                                "type" => if let Value::Str(s) = sv { server.instance_type = s.to_string(); },
                                "port" => if let Value::Int(i) = sv { server.port = i; },
                                _ => {}
                            }
                        }
                    }
                    topology.servers.push(server);
                }
            } else if key_str == "load_balancers" {
                // Parse load balancers Array
                let lb_array = rt.resolve_array(&v)?;
                for lb_val in lb_array {
                    let lb_hash = rt.resolve_hash(&lb_val)?;
                    let mut lb = LoadBalancerConfig::default();
                    for (lk, lv) in lb_hash {
                        if let Value::Sym(lsym) = lk {
                            match rt.resolve_sym(lsym) {
                                "name" => if let Value::Str(s) = lv { lb.name = s.to_string(); },
                                "routes" => {
                                    let r_array = rt.resolve_array(&lv)?;
                                    for r_val in r_array {
                                        let r_hash = rt.resolve_hash(&r_val)?;
                                        let mut path = String::new();
                                        let mut target = String::new();
                                        for (rk, rv) in r_hash {
                                            if let Value::Sym(rsym) = rk {
                                                match rt.resolve_sym(rsym) {
                                                    "path" => if let Value::Str(s) = rv { path = s.to_string(); },
                                                    "target" => if let Value::Str(s) = rv { target = s.to_string(); },
                                                    _ => {}
                                                }
                                            }
                                        }
                                        lb.routes.push((path, target));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    topology.load_balancers.push(lb);
                }
            }
        }
    }

    Some(topology)
}

// -----------------------------------------------------------------------------
// Printing Helper
// -----------------------------------------------------------------------------

fn print_topology(title: &str, topo: &InfraTopology) {
    println!("==================================================");
    println!("  IaC Deployment Plan: {}", title);
    println!("==================================================");
    println!("📦 Servers:");
    for s in &topo.servers {
        println!("  - {}:", s.name);
        println!("      Type: {}", s.instance_type);
        println!("      Port: {}", s.port);
    }
    println!("\n🔀 Load Balancers:");
    for lb in &topo.load_balancers {
        println!("  - {}:", lb.name);
        println!("      Routes:");
        for (path, target) in &lb.routes {
            println!("        {} -> {}", path, target);
        }
    }
    println!("==================================================\n");
}
