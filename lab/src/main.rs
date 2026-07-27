//! The lab's entry point — a menu of the experiments that currently exist.
//!
//! Run one with `cargo run -p praxis-lab -- <name>`. Each experiment is a module under
//! `src/experiments/`, added when you start it and deleted (with a NOTES.md verdict) when it
//! is done. This file is the only place they are listed.

fn main() {
    let name = std::env::args().nth(1);
    match name.as_deref() {
        // Register experiments here:
        //   Some("value-repr") => experiments::value_repr::run(),
        Some(other) => {
            eprintln!("praxis-lab: no experiment named `{other}`");
            eprintln!("run without arguments to list what exists");
        }
        None => {
            println!("praxis-lab — experiments before commitment (see lab/README.md)");
            println!();
            println!("  (none registered yet)");
            println!();
            println!("Start one:  create lab/src/experiments/<name>.rs, register it in main.rs,");
            println!("            and open its entry in lab/NOTES.md BEFORE you write the code.");
        }
    }
}
