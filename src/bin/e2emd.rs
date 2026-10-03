use clap::Parser;
use std::{path::PathBuf, time::Duration};
#[derive(Parser)]
#[command(about = "Local rules-only E2EM developer runtime")]
struct Args {
    #[cfg(unix)]
    #[arg(long)]
    socket: PathBuf,
    #[cfg(windows)]
    #[arg(long)]
    pipe: String,
    #[arg(long)]
    grants: PathBuf,
    #[arg(long, default_value_t = 300)]
    idle_seconds: u64,
}
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    #[cfg(unix)]
    {
        e2em_runtime::runtime::service::serve(
            &args.socket,
            &args.grants,
            Duration::from_secs(args.idle_seconds),
        )
        .await?;
        Ok(())
    }
    #[cfg(windows)]
    {
        e2em_runtime::runtime::service::serve(
            &args.pipe,
            &args.grants,
            Duration::from_secs(args.idle_seconds),
        )
        .await?;
        Ok(())
    }
}
