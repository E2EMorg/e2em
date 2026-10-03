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
            })
            .collect(),
    })
}

pub async fn serve(pipe: &str, grants_path: &Path, idle: Duration) -> std::io::Result<()> {
    read_grants(grants_path)?;
    let mut listener = e2em_platform::create_user_pipe(pipe, true)?;
    let scheduler = Arc::new(Scheduler::new(idle));
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
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
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
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = connection_loop(stream, 0, read_grants, grants_path, scheduler).await;
                });
            }
        }
    }
    Ok(())
}

async fn wait_pressure(
    monitor: &mut Option<e2em_platform::PressureMonitor>,
) -> std::io::Result<()> {
    match monitor {
        Some(monitor) => monitor.notified().await,
        None => std::future::pending().await,
    }
}
