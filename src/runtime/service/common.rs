//! Shared bounded framing, mutual grant proof and request dispatch.
use super::super::{scheduler::Scheduler, *};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, Semaphore},
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub principal: String,
    pub uid: u32,
    pub secret: String,
    #[serde(default)]
    pub model_management: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    pub provider: String,
    pub grants: Vec<Grant>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    principal: String,
    nonce: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    provider: String,
    nonce: String,
    proof: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    proof: String,
}

pub fn proof(
    secret: &str,
    role: &str,
    provider: &str,
    principal: &str,
    client: &str,
    server: &str,
) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts every key size");
    for part in [role, provider, principal, client, server] {
        mac.update(part.as_bytes());
        mac.update(&[0]);
    }
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn verify(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}
fn nonce() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> std::io::Result<Vec<u8>> {
    let size = reader.read_u32().await? as usize;
    if size == 0 || size > MAX_FRAME {
        return Err(std::io::Error::other("invalid frame size"));
    }
    let mut frame = vec![0; size];
    reader.read_exact(&mut frame).await?;
    Ok(frame)
}
pub async fn write_frame<W: AsyncWriteExt + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> std::io::Result<()> {
    let frame = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    if frame.len() > MAX_FRAME {
        return Err(std::io::Error::other("oversized reply"));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        writer.write_u32(frame.len() as u32).await?;
        writer.write_all(&frame).await?;
        writer.flush().await
    })
    .await?
}
pub(super) async fn connection_loop<S>(
    mut stream: S,
    uid: u32,
    read_grants: fn(&Path) -> std::io::Result<Grants>,
    grants_path: PathBuf,
    scheduler: Arc<Scheduler>,
    activity: Arc<super::super::update::Activity>,
    models: Option<super::super::models::Manager>,
) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let handshake = activity
        .enter()
        .ok_or_else(|| std::io::Error::other("runtime updating"))?;
    let grants = read_grants(&grants_path)?;
    let hello: Hello = serde_json::from_slice(
        &tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream)).await??,
    )
    .map_err(std::io::Error::other)?;
    if hello.nonce.len() != 64 || !hello.nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(std::io::Error::other("invalid nonce"));
    }
    let grant = grants
        .grants
        .iter()
        .find(|g| g.uid == uid && g.principal == hello.principal)
        .ok_or_else(|| std::io::Error::other("unauthorised peer"))?
        .clone();
    let server = nonce()?;
    let challenge = Challenge {
        provider: grants.provider.clone(),
        nonce: server.clone(),
        proof: proof(
            &grant.secret,
            "server",
            &grants.provider,
            &grant.principal,
            &hello.nonce,
            &server,
        ),
    };
    write_frame(&mut stream, &challenge).await?;
    let response: Proof = serde_json::from_slice(
        &tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream)).await??,
    )
    .map_err(std::io::Error::other)?;
    if !verify(
        &response.proof,
        &proof(
            &grant.secret,
            "client",
            &grants.provider,
            &grant.principal,
            &hello.nonce,
            &server,
        ),
    ) {
        return Err(std::io::Error::other("invalid peer proof"));
    }
    write_frame(
        &mut stream,
        &serde_json::json!({"authenticated": true, "provider": grants.provider}),
    )
    .await?;
    drop(handshake);
    let (mut reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(writer));
    let pending = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let inflight = Arc::new(Semaphore::new(8));
    let result = async {
        loop {
            let frame =
                tokio::time::timeout(Duration::from_secs(30), read_frame(&mut reader)).await??;
            let work = activity
                .enter()
                .ok_or_else(|| std::io::Error::other("runtime updating"))?;
            let current = read_grants(&grants_path)?;
            if current.provider != grants.provider
                || !current.grants.iter().any(|g| {
                    g.principal == grant.principal
                        && g.uid == uid
                        && verify(&g.secret, &grant.secret)
                })
            {
                scheduler.revoke(&grant.principal);
                return Err(std::io::Error::other("revoked grant"));
            }
            let call = match decode_call(&frame) {
                Ok(call) => call,
                Err(error) => {
                    let correlate = !error.call_id.is_empty();
                    write_frame(
                        &mut *writer.lock().await,
                        &Response {
                            call_id: error.call_id,
                            reply: Reply::Error {
                                error_code: error.error_code,
                            },
                        },
                    )
                    .await?;
                    if correlate {
                        continue;
                    }
                    return Err(std::io::Error::other("invalid call"));
                }
            };
            let reply = if call.api_version != API_VERSION {
                Reply::Error {
                    error_code: ErrorCode::UnsupportedVersion,
                }
            } else if !identifier(&call.call_id) {
                Reply::Error {
                    error_code: ErrorCode::InvalidRequest,
                }
            } else {
                match call.operation {
                    Operation::Models => match models.as_ref().and_then(|m| m.status().ok()) {
                        Some(status) => Reply::Models {
                            default_model: status["default"].as_str().unwrap_or("gandalf").into(),
                            models: serde_json::from_value(status["models"].clone())
                                .unwrap_or_default(),
                        },
                        None => Reply::Error {
                            error_code: ErrorCode::ModelUnavailable,
                        },
                    },
                    Operation::InstallModel {
                        source,
                        name,
                        auto_update,
                    } => {
                        if !current
                            .grants
                            .iter()
                            .any(|g| g.principal == grant.principal && g.model_management)
                        {
                            Reply::Error {
                                error_code: ErrorCode::UnsupportedPolicy,
                            }
                        } else if let Some(manager) = models.clone() {
                            if source.len() > 2048 || !super::super::model::name(&name) {
                                Reply::Error {
                                    error_code: ErrorCode::InvalidRequest,
                                }
                            } else {
                                let installed = tokio::task::spawn_blocking(move || {
                                    manager.install(&source, &name, auto_update)?;
                                    manager.status()
                                })
                                .await;
                                match installed {
                                    Ok(Ok(status)) => Reply::Models {
                                        default_model: status["default"]
                                            .as_str()
                                            .unwrap_or("gandalf")
                                            .into(),
                                        models: serde_json::from_value(status["models"].clone())
                                            .unwrap_or_default(),
                                    },
                                    _ => Reply::Error {
                                        error_code: ErrorCode::ModelUnavailable,
                                    },
                                }
                            }
                        } else {
                            Reply::Error {
                                error_code: ErrorCode::ModelUnavailable,
                            }
                        }
                    }
                    Operation::Capabilities => Reply::Capabilities {
                        capabilities: scheduler.capabilities(),
                    },
                    Operation::ValidatePolicy { policy } => {
                        match scheduler.validate_policy(&grant.principal, policy) {
                            Ok(policy_ref) => Reply::Policy { policy_ref },
                            Err(error_code) => Reply::Error { error_code },
                        }
                    }
                    Operation::Cancel { request_id } => Reply::Cancelled {
                        accepted: scheduler.cancel(&grant.principal, &request_id),
                    },
                    Operation::Assess { request } => {
                        let id = request.request_id.clone();
                        let permit = match inflight.clone().try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                write_frame(
                                    &mut *writer.lock().await,
                                    &Response {
                                        call_id: call.call_id,
                                        reply: Reply::Error {
                                            error_code: ErrorCode::ResourceExhausted,
                                        },
                                    },
                                )
                                .await?;
                                continue;
                            }
                        };
                        match scheduler.submit(&grant.principal, *request) {
                            Ok(receive) => {
                                let pending_key = (call.call_id.clone(), id.clone());
                                pending.lock().await.insert(pending_key.clone());
                                let writer = writer.clone();
                                let pending = pending.clone();
                                tokio::spawn(async move {
                                    let _work = work;
                                    let _permit = permit;
                                    let result = receive.await;
                                    let reply = match result {
                                        Ok(assessment) => Reply::Assessment { assessment },
                                        _ => Reply::Error {
                                            error_code: ErrorCode::ModelUnavailable,
                                        },
                                    };
                                    let _ = write_frame(
                                        &mut *writer.lock().await,
                                        &Response {
                                            call_id: call.call_id,
                                            reply,
                                        },
                                    )
                                    .await;
                                    pending.lock().await.remove(&pending_key);
                                });
                                continue;
                            }
                            Err(error_code) => Reply::Error { error_code },
                        }
                    }
                }
            };
            write_frame(
                &mut *writer.lock().await,
                &Response {
                    call_id: call.call_id,
                    reply,
                },
            )
            .await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    }
    .await;
    for (_, id) in pending.lock().await.iter() {
        scheduler.cancel(&grant.principal, id);
    }
    result
}
