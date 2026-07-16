//! `sag-node` binary — thin entrypoint. The seam and its tests live in `lib.rs`;
//! this just proves the binary builds and links. Real inventory + inference-proxy
//! wiring (and `sag-node bootstrap`) arrive with the bootstrap step.

fn main() -> anyhow::Result<()> {
    println!(
        "sag-node {} — pre-alpha skeleton; not serving yet.",
        env!("CARGO_PKG_VERSION")
    );
    println!("Run ./setup-node.sh to register this box; see README.md.");
    Ok(())
}
