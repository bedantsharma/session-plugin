//! Points the embedded dashboard (`BRIDGE_WEB_DIST`) and desktop card
//! (`BRIDGE_CARD_DIST`) at the web build, or at a placeholder page when there is none.

use std::path::PathBuf;

#[path = "src/ui/dist.rs"]
mod dist;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it"));
    let web = manifest.join("../../web");
    for (var, folder, index) in [
        ("BRIDGE_WEB_DIST", "dist", "index.html"),
        ("BRIDGE_CARD_DIST", "dist-card", "card.html"),
    ] {
        let built = web.join(folder);
        println!("cargo:rerun-if-changed={}", built.display());
        let chosen = dist::choose(&built, index, &out.join(format!("placeholder-{folder}")))
            .expect("writing the placeholder page");
        let chosen = chosen.canonicalize().unwrap_or(chosen);
        println!("cargo:rustc-env={var}={}", chosen.display());
    }
}
