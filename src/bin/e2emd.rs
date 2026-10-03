use clap::Parser;
use e2em_runtime::runtime::{
    service,
    update::{self, Channel, Config, Store, Updater},
};
use std::{path::PathBuf, sync::Arc, time::Duration};
#[derive(Parser)]
#[command(name = "e2emd", version, about = "Local E2EM developer runtime")]
struct Args {
    /// Use the diagnostic rules backend instead of the installed model.
    #[arg(long)]
    rules_only: bool,
    /// Disable all runtime/model network activity.
    #[arg(long)]
    offline: bool,
    #[arg(long)]
    model_config: Option<PathBuf>,
    #[arg(long)]
    model_init: bool,
    #[arg(long, requires = "model_init")]
    model_worker: Option<PathBuf>,
    #[arg(long, requires = "model_init")]
    model_library: Option<PathBuf>,
    #[arg(long)]
    model_install: Option<String>,
    #[arg(long, default_value = "gandalf")]
    model_name: String,
    #[arg(long)]
    model_no_update: bool,
    #[arg(long)]
    model_use: Option<String>,
    #[arg(long)]
    model_status: bool,
    #[arg(long)]
    model_check: bool,
    #[arg(long)]
    model_rollback: Option<String>,
    #[cfg(unix)]
    #[arg(long, required_unless_present_any = ["update_status", "update_check_now", "model_init", "model_install", "model_use", "model_status", "model_check", "model_rollback"])]
    socket: Option<PathBuf>,
    #[cfg(windows)]
    #[arg(long, required_unless_present_any = ["update_status", "update_check_now", "model_init", "model_install", "model_use", "model_status", "model_check", "model_rollback"])]
    pipe: Option<String>,
    #[arg(long)]
    grants: PathBuf,
    #[arg(long, default_value_t = 300)]
    idle_seconds: u64,
    /// Check and apply runtime updates in the background during idle periods.
    #[arg(long)]
    auto_update: bool,
    /// Disable background updates and run the installed base executable.
    #[arg(long)]
    no_auto_update: bool,
    #[arg(long, value_enum, default_value_t = Channel::Preview)]
    update_channel: Channel,
    #[arg(long, default_value_t = 21600, value_parser = clap::value_parser!(u64).range(300..=604800))]
    update_check_seconds: u64,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=86400))]
    update_idle_seconds: u64,
    /// Show the background updater's persisted JSON status.
    #[arg(long, conflicts_with = "update_check_now")]
    update_status: bool,
    /// Request a check at the next idle opportunity (also works before startup).
    #[arg(long)]
    update_check_now: bool,
    #[arg(long, hide = true)]
    update_child: bool,
    #[arg(long, hide = true, requires = "update_child")]
    update_parent: Option<u32>,
    #[arg(long, hide = true, requires = "update_child")]
    update_ready_file: Option<PathBuf>,
}
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    service::read_grants(&args.grants)?;
    let model_path = args.model_config.clone().unwrap_or_else(|| {
        args.grants
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("models.json")
    });
    let manager = e2em_runtime::runtime::models::Manager::new(model_path);
    if args.model_init {
        manager.initialize(
            args.model_worker
                .clone()
                .ok_or("--model-worker is required")?,
            args.model_library
                .clone()
                .ok_or("--model-library is required")?,
        )?;
    }
    if let Some(source) = &args.model_install {
        if args.offline && !std::path::Path::new(source).is_dir() {
            return Err("offline installation requires a local model package".into());
        }
        manager.install(source, &args.model_name, !args.model_no_update)?;
    }
    if let Some(alias) = &args.model_use {
        manager.set_default(alias)?;
    }
    if let Some(alias) = &args.model_rollback {
        manager.rollback(alias)?;
    }
    if args.model_check {
        if args.offline {
            return Err("model checks are disabled in offline mode".into());
        }
        manager.check_updates(true, &|| Ok(()))?;
    }
    if args.model_status
        || args.model_init
        || args.model_install.is_some()
        || args.model_use.is_some()
        || args.model_check
        || args.model_rollback.is_some()
    {
        if args.model_status || args.model_install.is_some() {
            println!("{}", serde_json::to_string_pretty(&manager.status()?)?);
        }
        return Ok(());
    }
    let directory = update::supervisor::directory(&args.grants)?;
    if args.update_status || args.update_check_now {
        let store = Store::open(&directory)?;
        if args.update_status {
            let status = match store.read::<update::Status>("status.json") {
                Ok(status) => status,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => update::Status {
                    stage: "not_started".into(),
                    ..Default::default()
                },
                Err(e) => return Err(e.into()),
            };
            println!("{}", serde_json::to_string_pretty(&status)?);
        } else {
            store.write("check-request.json", &update::now())?;
            println!("Update check requested for the next idle opportunity.");
        }
        return Ok(());
    }
    let mut enabled = args.auto_update && !args.no_auto_update && !args.offline;
    let config = Config {
        directory,
        channel: args.update_channel,
        interval: Duration::from_secs(args.update_check_seconds),
        idle: Duration::from_secs(args.update_idle_seconds),
        ready_file: args.update_ready_file,
        parent: args.update_parent,
    };
    if enabled && !args.update_child {
        match update::supervisor::run(config.clone(), std::env::args_os().skip(1).collect()).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("updater unavailable; continuing with installed runtime: {error}");
                enabled = false;
            }
        }
    }
    let updater = if enabled {
        Some(Updater::new(config, Arc::default())?)
    } else {
        None
    };
    #[cfg(unix)]
    let restarting = service::serve_with_models(
        args.socket.as_deref().unwrap(),
        &args.grants,
        Duration::from_secs(args.idle_seconds),
        updater,
        (!args.rules_only).then_some(manager),
        args.offline,
    )
    .await?;
    #[cfg(windows)]
    let restarting = service::serve_with_models(
        args.pipe.as_deref().unwrap(),
        &args.grants,
        Duration::from_secs(args.idle_seconds),
        updater,
        (!args.rules_only).then_some(manager),
        args.offline,
    )
    .await?;
    if restarting {
        std::process::exit(update::UPDATE_EXIT);
    }
    Ok(())
}
