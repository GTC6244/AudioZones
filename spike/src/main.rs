//! AudioZones de-risking spike — answers the 4 questions from the design doc.
//!
//! ┌──────────────────────────────────────────────────────────────────────┐
//! │ Q1 READ PATH      connect to the pw main loop, enumerate the registry, │
//! │                   print live add/remove events as you tinker.          │
//! │ Q2 COMMAND PATH   issue a mutation (create a link) from a SECOND        │
//! │                   thread, marshaled onto the pw-loop thread.           │
//! │ Q3 WP COEXISTENCE after we create the link, does it STICK — or does    │
//! │                   WirePlumber revert it within seconds? (watch for a   │
//! │                   matching "- removed" line right after we create it.) │
//! │ Q4 VOLUME MODEL   see the README — investigated with wpctl/pw-dump,    │
//! │                   not from this binary.                                │
//! └──────────────────────────────────────────────────────────────────────┘
//!
//! This is a SPIKE: minimal, throwaway, single-threaded loop + one worker thread.
//! The pw main loop is !Send, so it owns its thread and is the only writer — the
//! same single-writer model the real server will use.
//!
//! NOTE: written against the `pipewire` crate ~0.8. The exact method signatures
//! (e.g. `MainLoop::new`) drift between versions — if it doesn't compile, the
//! fix is almost always a one-line signature tweak, not a design problem.

use std::cell::RefCell;
use std::thread;
use std::time::Duration;

use pipewire as pw;
use pw::{context::Context, main_loop::MainLoop};

/// Command sent from the worker thread onto the pw-loop thread.
enum Cmd {
    /// Create a link: (output_node, output_port, input_node, input_port) — numeric ids.
    CreateLink(u32, u32, u32, u32),
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    pw::init();

    println!("=== AudioZones spike — watch the 4 questions ===");
    println!("Q1: registry adds/removes print below as '+'/'-' lines.");
    println!("    Open qpwgraph and add/remove a connection — you should see events here.");

    let main_loop = MainLoop::new(None)?;
    let context = Context::new(&main_loop)?;
    let core = context.connect(None)?;
    let registry = core.get_registry()?;

    // --- Q1 + Q3: print every object as it appears and disappears -----------
    // Links are the interesting ones: if WirePlumber reverts a link we create,
    // a "- removed id=N (was a Link)" will appear seconds after we create it.
    let _reg_listener = registry
        .add_listener_local()
        .global(|g| {
            // Pull a couple of identifying props if present.
            let name = g
                .props
                .and_then(|p| {
                    p.get("node.name")
                        .or_else(|| p.get("port.name"))
                        .or_else(|| p.get("object.path"))
                })
                .unwrap_or("");
            println!("  + {:<14} id={:<5} {}", format!("{:?}", g.type_), g.id, name);
        })
        .global_remove(|id| {
            println!("  - removed       id={}", id);
        })
        .register();

    // --- Q2: a worker thread sends a command onto the loop thread -----------
    // pw::channel wakes the loop; the closure below runs ON the loop thread.
    let (sender, receiver) = pw::channel::channel::<Cmd>();

    // Keep created proxies alive — dropping a Link proxy destroys the link.
    let core_for_loop = core.clone();
    // RefCell: receiver.attach takes an `Fn` closure, so we mutate through a shared ref.
    let created_links: RefCell<Vec<pw::link::Link>> = RefCell::new(Vec::new());
    let _recv = receiver.attach(main_loop.loop_(), move |cmd| match cmd {
        Cmd::CreateLink(on, op, inn, ip) => {
            println!("Q2: [loop thread] creating link {on}:{op} -> {inn}:{ip}");
            let props = pw::properties::properties! {
                *pw::keys::LINK_OUTPUT_NODE => on.to_string(),
                *pw::keys::LINK_OUTPUT_PORT => op.to_string(),
                *pw::keys::LINK_INPUT_NODE  => inn.to_string(),
                *pw::keys::LINK_INPUT_PORT  => ip.to_string(),
                // linger=false so the link dies with this process (it's a spike).
                *pw::keys::OBJECT_LINGER => "false",
            };
            match core_for_loop.create_object::<pw::link::Link>("link-factory", &props) {
                Ok(link) => {
                    println!("Q2: link object created. Q3: WATCH — does it stick?");
                    println!("    If a '- removed' for this link appears in a few seconds,");
                    println!("    WirePlumber is reverting it -> server must talk to WP, not raw pw.");
                    created_links.borrow_mut().push(link);
                }
                Err(e) => println!("Q2: create_object FAILED: {e:?}"),
            }
        }
    });

    // Worker thread: wait, then ask the loop to create the link.
    // Pick the 4 ids from `pw-dump` / the registry output, pass via env.
    let after = env_u32("SPIKE_LINK_AFTER_SECS").unwrap_or(6);
    let link_args = (
        env_u32("SPIKE_OUT_NODE"),
        env_u32("SPIKE_OUT_PORT"),
        env_u32("SPIKE_IN_NODE"),
        env_u32("SPIKE_IN_PORT"),
    );
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(after as u64));
        match link_args {
            (Some(on), Some(op), Some(inn), Some(ip)) => {
                println!("Q2: [worker thread] sending CreateLink to loop thread...");
                let _ = sender.send(Cmd::CreateLink(on, op, inn, ip));
            }
            _ => println!(
                "Q2/Q3 SKIPPED: set SPIKE_OUT_NODE/PORT and SPIKE_IN_NODE/PORT \
                 (numeric ids from the '+' output above) to test mutation."
            ),
        }
    });

    println!("Running. Ctrl-C to quit.\n");
    main_loop.run(); // blocks; this thread is the single pw writer.
    Ok(())
}
