use clap::Parser;
use e2em_runtime::runtime::{
    service,
    update::{self, Channel, Config, Store, Updater},
};
use std::{path::PathBuf, sync::Arc, time::Duration};
#[derive(Parser)]
#[command(name = "e2emd", version, about = "Local E2EM runtime")]
struct Args {
    /// Open guided setup in your browser.
    #[arg(long, conflicts_with_all = ["setup_headless", "setup_status"])]
    setup: bool,
    /// Complete guided setup without a browser (for managed installations).
    #[arg(long, conflicts_with = "setup_status")]
    setup_headless: bool,
    /// Check the model and authenticated runtime connection.
    #[arg(long)]
    setup_status: bool,
    /// Print the local setup URL without opening a browser.
    #[arg(long, requires = "setup")]
    setup_no_browser: bool,
    /// Request consent to connect this application in the setup screen.
    #[arg(long, requires = "setup")]
    setup_app: Option<String>,
    #[arg(long, hide = true)]
    setup_home: Option<PathBuf>,
    /// Use a local deployment package during unattended setup.
    #[arg(long, requires = "setup_headless")]
    setup_model_source: Option<String>,
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
    #[arg(long, required_unless_present_any = ["setup", "setup_headless", "setup_status", "update_status", "update_check_now", "model_init", "model_install", "model_use", "model_status", "model_check", "model_rollback"])]
    socket: Option<PathBuf>,
    #[cfg(windows)]
    #[arg(long, required_unless_present_any = ["setup", "setup_headless", "setup_status", "update_status", "update_check_now", "model_init", "model_install", "model_use", "model_status", "model_check", "model_rollback"])]
    pipe: Option<String>,
    #[arg(long, required_unless_present_any = ["setup", "setup_headless", "setup_status"])]
    grants: Option<PathBuf>,
    #[arg(long, default_value_t = 300)]
    idle_seconds: u64,
    /// Check and apply runtime updates in the background during idle periods.
    #[arg(long)]
    auto_update: bool,
    /// Disable background updates and run the installed base executable.
    #[arg(long)]
    no_auto_update: bool,
    #[arg(long, value_enum, default_value_t = Channel::Stable)]
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
    if args.setup || args.setup_headless || args.setup_status {
        let context = e2em_runtime::runtime::setup::Context::discover(args.setup_home)?
            .with_app(args.setup_app)?;
        if args.setup_status {
            let status = context.status().await;
            println!("{}", serde_json::to_string_pretty(&status)?);
            if status["stage"] != "ready" {
                std::process::exit(1);
            }
        } else if args.setup_headless {
            context
                .complete(
                    e2em_runtime::runtime::setup::Preferences {
                        offline: args.offline,
                        auto_update: !args.no_auto_update && !args.offline,
                        start_at_login: true,
                    },
                    args.setup_model_source,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&context.status().await)?);
        } else {
            e2em_runtime::runtime::setup::serve(context, !args.setup_no_browser).await?;
        }
        return Ok(());
    }
    let grants = args
        .grants
        .as_ref()
        .expect("clap requires grants for runtime commands");
    service::read_grants(grants)?;
    let model_path = args.model_config.clone().unwrap_or_else(|| {
        grants
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
    let directory = update::supervisor::directory(grants)?;
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
    if !args.rules_only {
        manager.ensure_default(args.offline, !args.model_no_update && !args.no_auto_update)?;
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
        grants,
        Duration::from_secs(args.idle_seconds),
        updater,
        (!args.rules_only).then_some(manager),
        args.offline,
    )
    .await?;
    #[cfg(windows)]
    let restarting = service::serve_with_models(
        args.pipe.as_deref().unwrap(),
        grants,
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
