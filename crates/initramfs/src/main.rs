//! Code for bootc that goes into the initramfs.
//! At the current time, this is mostly just a no-op.
// SPDX-License-Identifier: Apache-2.0 OR MIT

use anyhow::Result;

fn setup_root() -> Result<()> {
    let _ = std::fs::metadata("/sysroot/usr")?;
    println!("setup OK");
    Ok(())
}

fn main() -> Result<()> {
    let v = std::env::args().collect::<Vec<_>>();
    let args = match v.as_slice() {
        [] => anyhow::bail!("Missing argument".to_string()),
        [_, rest @ ..] => rest,
    };
    match args {
        [] => anyhow::bail!("Missing argument".to_string()),
        [s] if s == "setup-root" => setup_root(),
        [o, ..] => anyhow::bail!(format!("Unknown command {o}")),
    }
}
