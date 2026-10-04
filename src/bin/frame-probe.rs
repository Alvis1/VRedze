#[cfg(feature = "decode")]
use vredze::decode;
use vredze::inventory;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "On-device decode feasibility checks for VRedze")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect this machine. Listings never constitute a decoding pass.
    Inventory,
    /// Decode a local sample using an explicit hardware backend; no software fallback.
    #[cfg(feature = "decode")]
    Decode(decode::Options),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Inventory => {
            println!("{}", serde_json::to_string_pretty(&inventory::collect())?);
        }
        #[cfg(feature = "decode")]
        Command::Decode(options) => {
            let report = decode::run(options)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.sample_decoded {
                std::process::exit(2);
            }
        }
    }
    Ok(())
}
