//! Declared build producer for browser schemas and Rust-encoded wire samples.
use harness::server::browser_contract;
use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let output = PathBuf::from(arguments.next().ok_or("expected output directory")?);
    if arguments.next().is_some() {
        return Err("expected only an output directory".into());
    }
    fs::create_dir_all(&output)?;
    fs::write(
        output.join("schemas.json"),
        serde_json::to_vec_pretty(&browser_contract::schemas())?,
    )?;
    fs::write(
        output.join("wire-samples.json"),
        serde_json::to_vec_pretty(&browser_contract::wire_samples())?,
    )?;
    Ok(())
}
