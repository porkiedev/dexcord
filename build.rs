use std::io::Result;

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=src/PreloadedUserSettings.proto");
    prost_build::compile_protos(&["src/PreloadedUserSettings.proto"], &["src/"])?;
    Ok(())
}
