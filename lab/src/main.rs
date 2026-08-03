//! The lab's entry point — a menu of the experiments that currently exist.
//!
//! Run one with `cargo run -p praxis-lab -- <name>`. Each experiment is a module under
//! `src/experiments/`, added when you start it and deleted (with a NOTES.md verdict) when it
//! is done. This file is the only place they are listed.

mod experiments;

fn main() -> std::process::ExitCode {
    let name = std::env::args().nth(1);
    match name.as_deref() {
        Some("gc-pressure") => {
            return experiments::gc_pressure::run(std::env::args().nth(2).as_deref());
        }
        Some("hot-shapes") => {
            return experiments::hot_shapes::run(std::env::args().nth(2).as_deref());
        }
        Some("run-module") => {
            return experiments::run_module::run(
                std::env::args().nth(2).as_deref(),
                std::env::args().nth(3).as_deref(),
            );
        }
        Some("nesting-cost") => {
            return experiments::nesting_cost::run(std::env::args().nth(2).as_deref());
        }
        Some(other) => {
            eprintln!("praxis-lab: no experiment named `{other}`");
            eprintln!("run without arguments to list what exists");
            return std::process::ExitCode::FAILURE;
        }
        None => {
            println!("praxis-lab — experiments before commitment (see lab/README.md)");
            println!();
            println!("  gc-pressure    what the property-escapes bucket costs in time and memory");
            println!("  hot-shapes     where the interpreter's time and allocations go, per shape");
            println!("  run-module     run a real module graph off the disk");
            println!("  nesting-cost   how much stack a nesting level costs, per shape");
            println!();
            println!("Start one:  create lab/src/experiments/<name>.rs, register it in main.rs,");
            println!("            and open its entry in lab/NOTES.md BEFORE you write the code.");
        }
    }
    std::process::ExitCode::SUCCESS
}
