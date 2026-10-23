//! Embeds the application icon into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../stewardget-gui/assets/stewardget.rc");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Best effort: a missing resource compiler only costs the Explorer icon.
        if let Err(e) =
            embed_resource::compile("../stewardget-gui/assets/stewardget.rc", embed_resource::NONE).manifest_optional()
        {
            println!("cargo:warning=executable icon not embedded: {e}");
        }
    }
}
