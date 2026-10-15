fn main() {
    // A ceiling on the module's linear memory, set where the module is linked so that no host can
    // forget it: 512 MiB is eight times the engine's default heap budget, which leaves the budget
    // to refuse first and this to stop whatever the budget does not count.
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("wasm") {
        println!("cargo:rustc-link-arg=--max-memory=536870912");
    }
}
