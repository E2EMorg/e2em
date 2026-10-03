//! Per-user named-pipe prototype; app authority still requires an explicit grant.
use super::super::{identifier, scheduler::Scheduler};
use super::common::{Grant, Grants, connection_loop};
use serde::Deserialize;
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowsGrant {
    principal: String,
    sid: String,
    secret: String,
    #[serde(default)]
    model_management: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowsGrants {
    provider: String,
    grants: Vec<WindowsGrant>,
}

pub fn read_grants(path: &Path) -> std::io::Result<Grants> {
    let bytes = e2em_platform::read_private_file(path)?;
    let raw: WindowsGrants = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    let sid = e2em_platform::current_user_sid()?;
    let mut principals = std::collections::HashSet::new();
    if !identifier(&raw.provider)
        || raw.grants.len() > 64
        || raw.grants.iter().any(|g| {
            g.sid != sid
                || !identifier(&g.principal)
                || !principals.insert(&g.principal)
                || g.secret.len() != 64
                || !g.secret.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err(std::io::Error::other("invalid Windows grants"));
    }
    // The shared dispatcher receives a verified same-user connection. uid=0 is
    // an internal adapter key; it is never accepted from Windows grant JSON.
    Ok(Grants {
        provider: raw.provider,
        grants: raw
            .grants
            .into_iter()
            .map(|g| Grant {
                principal: g.principal,
                uid: 0,
                secret: g.secret,
                model_management: g.model_management,
            })
            .collect(),
    })
}

pub async fn serve(pipe: &str, grants_path: &Path, idle: Duration) -> std::io::Result<()> {
    serve_with_updates(pipe, grants_path, idle, None)
        .await
        .map(|_| ())
}
pub async fn serve_with_updates(
    pipe: &str,
    grants_path: &Path,
    idle: Duration,
    updater: Option<super::super::update::Updater>,
) -> std::io::Result<bool> {
    serve_with_models(pipe, grants_path, idle, updater, None, false).await
}
pub async fn serve_with_models(
    pipe: &str,
    grants_path: &Path,
    idle: Duration,
    mut updater: Option<super::super::update::Updater>,
    models: Option<super::super::models::Manager>,
    offline: bool,
) -> std::io::Result<bool> {
    read_grants(grants_path)?;
    let mut listener = e2em_platform::create_user_pipe(pipe, true)?;
    let scheduler = Arc::new(match &models {
        Some(manager) => Scheduler::with_models(idle, manager.clone())?,
        None => Scheduler::new(idle),
    });
    let activity = updater
        .as_ref()
        .map(|u| u.activity.clone())
        .unwrap_or_default();
    if let Some(updater) = &updater {
        updater.mark_ready()?;
    }
    let mut update_tick = tokio::time::interval(Duration::from_secs(1));
    let mut model_tick = tokio::time::interval(Duration::from_secs(60));
    let model_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut model_task: Option<tokio::task::JoinHandle<std::io::Result<()>>> = None;
    let mut tasks = tokio::task::JoinSet::new();
    let mut restarting = false;
    let connections = Arc::new(Semaphore::new(32));
    let mut pressure = match e2em_platform::PressureMonitor::open() {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            eprintln!(
                "automatic memory-pressure monitor unavailable: {error}; idle eviction remains enabled"
            );
            None
        }
    };
    let mut console_break = tokio::signal::windows::ctrl_break()?;
    let mut console_interrupt = tokio::signal::windows::ctrl_c()?;
    loop {
        tokio::select! {
            _ = model_tick.tick(), if models.is_some() && !offline => {
                if model_task.as_ref().is_some_and(|t| t.is_finished()) {
                    if let Some(task) = model_task.take() && let Ok(Err(error)) = task.await { eprintln!("model update deferred: {error}"); }
                }
                if model_task.is_none() && activity.idle_for(Duration::from_secs(60)) {
                    let manager = models.as_ref().unwrap().clone(); let activity = activity.clone(); let stop = model_stop.clone();
                    model_task = Some(tokio::task::spawn_blocking(move || manager.check_updates(false, &|| {
                        if !stop.load(std::sync::atomic::Ordering::Acquire) && activity.idle_for(Duration::from_secs(60)) { Ok(()) }
                        else { Err(std::io::Error::from(std::io::ErrorKind::Interrupted)) }
                    })));
                }
            },
            _ = update_tick.tick(), if updater.is_some() => {
                if updater.as_ref().is_some_and(|u| !u.parent_alive()) { break; }
                if updater.as_mut().is_some_and(|u| u.poll()) { restarting = true; break; }
            },
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            _ = console_interrupt.recv() => break,
            _ = console_break.recv() => break,
            event = wait_pressure(&mut pressure) => {
                match event {
                    Ok(()) => { scheduler.memory_pressure(); },
                    Err(error) => {
                        eprintln!("automatic memory-pressure monitor stopped: {error}");
                        pressure = None;
                    }
                }
            },
            connected = listener.connect() => {
                connected?;
                // Keep a server handle alive continuously so another process
                // cannot take over the namespace between accepted clients.
                let next = e2em_platform::create_user_pipe(pipe, false)?;
                let stream = std::mem::replace(&mut listener, next);
                let Ok(permit) = connections.clone().try_acquire_owned() else { continue; };
                if e2em_platform::verify_pipe_peer(&stream).is_err() { continue; }
                let scheduler = scheduler.clone();
                let grants_path = grants_path.to_owned();
                let activity = activity.clone();
                let models = models.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let _ = connection_loop(stream, 0, read_grants, grants_path, scheduler, activity, models).await;
                });
            }
        }
    }
    tasks.shutdown().await;
    model_stop.store(true, std::sync::atomic::Ordering::Release);
    Ok(restarting)
}

async fn wait_pressure(
    monitor: &mut Option<e2em_platform::PressureMonitor>,
) -> std::io::Result<()> {
    match monitor {
        Some(monitor) => monitor.notified().await,
        None => std::future::pending().await,
    }
}
