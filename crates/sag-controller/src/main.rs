//! `sag-controller` binary — thin entrypoint. The seam and its tests live in
//! `lib.rs`; this just proves the binary builds and links. Real scheduling,
//! persistence, and the LAN (`axum` + JSON) surface arrive with later build steps.

fn main() -> anyhow::Result<()> {
    println!(
        "sag-controller {} — pre-alpha skeleton; not serving yet.",
        env!("CARGO_PKG_VERSION")
    );
    println!("See the Quickstart in README.md; design in ARCHITECTURE.md.");
    Ok(())
}
