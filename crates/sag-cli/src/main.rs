//! `sag-cli` binary — thin entrypoint. The seam and its tests live in `lib.rs`;
//! this just proves the binary builds and links. Real argument parsing (`clap`)
//! and the `attach` TUI (`ratatui`) arrive with later build steps.

fn main() -> anyhow::Result<()> {
    println!(
        "sag {} — pre-alpha skeleton; the runtime is not wired up yet.",
        env!("CARGO_PKG_VERSION")
    );
    println!("Getting started: see the Quickstart in README.md.");
    Ok(())
}
