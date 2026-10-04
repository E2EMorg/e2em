//! Bounded loopback UI. The random path is a local capability, never a public API.
use super::{Context, Preferences, platform, random};
use serde_json::{Value, json};
use std::{io, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify, Semaphore},
};

struct Session {
    context: Context,
    host: String,
    token: String,
    operation: Arc<Mutex<()>>,
    shutdown: Notify,
}

pub async fn serve(context: Context, open_browser: bool) -> io::Result<()> {
    let lock = match context.lock() {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            let previous: Value = context.store()?.read("session.json")?;
            let url = previous["url"]
                .as_str()
                .ok_or_else(|| io::Error::other("E2EM Setup is already open."))?;
            let url = context.screen_url(url);
            if open_browser {
                platform::browser(&url)?;
            }
            println!("E2EM Setup is already open: {url}");
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let host = listener.local_addr()?.to_string();
    let token = random()?;
    let url = format!("http://{host}/{token}/");
    context
        .store()?
        .write("session.json", &json!({"url":url}))?;
    *context.progress.lock().expect("setup progress lock") = context.status().await;
    let screen_url = context.screen_url(&url);
    println!("Open E2EM Setup: {screen_url}");
    if open_browser && let Err(error) = platform::browser(&screen_url) {
        eprintln!("Open the setup URL above in your browser: {error}");
    }
    let session = Arc::new(Session {
        context,
        host,
        token,
        operation: Arc::default(),
        shutdown: Notify::new(),
    });
    let connections = Arc::new(Semaphore::new(8));
    loop {
        tokio::select! {
            _ = session.shutdown.notified() => break,
            connection = tokio::time::timeout(Duration::from_secs(600),listener.accept()) => {
                let (stream,_) = match connection {
                    Ok(result) => result?,
                    Err(_) if session.operation.try_lock().is_ok() => break,
                    Err(_) => continue,
                };
                let Ok(permit) = connections.clone().try_acquire_owned() else { continue; };
                let session = session.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = tokio::time::timeout(Duration::from_secs(6), handle(stream,session)).await;
                });
            }
        }
    }
    drop(lock);
    Ok(())
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

async fn request(stream: &mut TcpStream, session: &Session) -> io::Result<Request> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() >= 4096 {
            return Err(io::Error::other("Oversized request headers."));
        }
        let mut buffer = [0; 512];
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(io::Error::other("Incomplete request."));
        }
        bytes.extend_from_slice(&buffer[..count]);
    };
    if header_end > 4096 {
        return Err(io::Error::other("Oversized request headers."));
    }
    let headers = std::str::from_utf8(&bytes[..header_end]).map_err(io::Error::other)?;
    let mut lines = headers.split("\r\n");
    let start: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    if start.len() != 3 || start[2] != "HTTP/1.1" {
        return Err(io::Error::other("Invalid request."));
    }
    let method = start[0].to_owned();
    let prefix = format!("/{}/", session.token);
    let path = start[1]
        .split('?')
        .next()
        .unwrap_or("")
        .strip_prefix(&prefix)
        .ok_or_else(|| io::Error::other("Invalid setup session."))?
        .to_owned();
    let mut host = None;
    let mut origin = None;
    let mut length = None;
    let mut content_type = None;
    for line in lines.filter(|s| !s.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("Invalid header."))?;
        let name = name.to_ascii_lowercase();
        let slot = match name.as_str() {
            "host" => &mut host,
            "origin" => &mut origin,
            "content-length" => &mut length,
            "content-type" => &mut content_type,
            "transfer-encoding" => {
                return Err(io::Error::other("Chunked requests are not supported."));
            }
            _ => continue,
        };
        if slot.replace(value.trim().to_owned()).is_some() {
            return Err(io::Error::other("Duplicate header."));
        }
    }
    if host.as_deref() != Some(&session.host) {
        return Err(io::Error::other("Invalid setup host."));
    }
    let length: usize = length
        .unwrap_or_else(|| "0".into())
        .parse()
        .map_err(io::Error::other)?;
    if length > 1024 {
        return Err(io::Error::other("Oversized request body."));
    }
    if method == "POST"
        && (origin.as_deref() != Some(&format!("http://{}", session.host))
            || content_type.as_deref() != Some("application/json"))
    {
        return Err(io::Error::other(
            "Setup changes require a request from this setup screen.",
        ));
    }
    while bytes.len() < header_end + length {
        let mut buffer = [0; 1024];
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(io::Error::other("Incomplete request."));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(Request {
        method,
        path,
        body: bytes[header_end..header_end + length].to_vec(),
    })
}

async fn respond(
    stream: &mut TcpStream,
    code: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let headers = format!(
        "HTTP/1.1 {code}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

async fn handle(mut stream: TcpStream, session: Arc<Session>) -> io::Result<()> {
    let request = match request(&mut stream, &session).await {
        Ok(request) => request,
        Err(_) => {
            return respond(
                &mut stream,
                "403 Forbidden",
                "text/plain",
                b"This setup request was rejected.",
            )
            .await;
        }
    };
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "") => {
            return respond(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                include_bytes!("ui.html"),
            )
            .await;
        }
        ("GET", "app.js") => {
            return respond(
                &mut stream,
                "200 OK",
                "text/javascript; charset=utf-8",
                include_bytes!("app.js"),
            )
            .await;
        }
        ("GET", "style.css") => {
            return respond(
                &mut stream,
                "200 OK",
                "text/css; charset=utf-8",
                include_bytes!("style.css"),
            )
            .await;
        }
        ("GET", "status") => {
            let mut state = session
                .context
                .progress
                .lock()
                .expect("setup progress lock")
                .clone();
            if session.operation.try_lock().is_ok() && state["stage"] == "ready" {
                state = session.context.status().await;
            }
            return respond(
                &mut stream,
                "200 OK",
                "application/json",
                &serde_json::to_vec(&state)?,
            )
            .await;
        }
        ("POST", "start") => {
            let preferences: Preferences = match serde_json::from_slice(&request.body) {
                Ok(value) => value,
                Err(_) => {
                    return respond(
                        &mut stream,
                        "400 Bad Request",
                        "text/plain",
                        b"Invalid setup preferences.",
                    )
                    .await;
                }
            };
            let Ok(guard) = session.operation.clone().try_lock_owned() else {
                return respond(
                    &mut stream,
                    "409 Conflict",
                    "text/plain",
                    b"Setup is already in progress.",
                )
                .await;
            };
            let session = session.clone();
            session
                .context
                .progress("configuring", "Preparing your private runtime…");
            tokio::spawn(async move {
                let _guard = guard;
                let preferences = Preferences {
                    auto_update: preferences.auto_update && !preferences.offline,
                    ..preferences
                };
                let _ = session.context.perform(preferences, None).await;
            });
            return respond(&mut stream, "202 Accepted", "application/json", b"{}").await;
        }
        ("POST", "enrol") => {
            let Ok(_guard) = session.operation.try_lock() else {
                return respond(
                    &mut stream,
                    "409 Conflict",
                    "text/plain",
                    b"Wait for setup to finish.",
                )
                .await;
            };
            let input: Value = serde_json::from_slice(&request.body).map_err(io::Error::other)?;
            if session.context.status().await["stage"] != "ready" {
                return respond(
                    &mut stream,
                    "409 Conflict",
                    "text/plain",
                    b"Set up E2EM before connecting an app.",
                )
                .await;
            }
            let result = session
                .context
                .enrol(input["principal"].as_str().unwrap_or(""), true);
            let (code, body) = match result {
                Ok(path) => (
                    "200 OK",
                    json!({"message":"App connected. You can return to your application.","credential_path":path}),
                ),
                Err(error) => ("400 Bad Request", json!({"message":error.to_string()})),
            };
            return respond(
                &mut stream,
                code,
                "application/json",
                &serde_json::to_vec(&body)?,
            )
            .await;
        }
        ("POST", "close") if session.operation.try_lock().is_ok() => {
            respond(&mut stream, "200 OK", "application/json", b"{}").await?;
            session.shutdown.notify_one();
            return Ok(());
        }
        _ => (),
    }
    respond(
        &mut stream,
        "404 Not Found",
        "text/plain",
        b"Setup page not found.",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn accepted(headers: &str, path: &str, body: &str) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let context = Context {
            home: PathBuf::new(),
            root: PathBuf::new(),
            binary: PathBuf::new(),
            endpoint: String::new(),
            requested_app: None,
            progress: Arc::new(std::sync::Mutex::new(json!({}))),
        };
        let session = Session {
            context,
            host: address.to_string(),
            token: "secret".into(),
            operation: Arc::default(),
            shutdown: Notify::new(),
        };
        let headers = headers.replace("ORIGIN", &format!("http://{address}"));
        let text = format!(
            "POST /{path}/start HTTP/1.1\r\nHost: {address}\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream.write_all(text.as_bytes()).await.unwrap();
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        let result = request(&mut stream, &session).await.is_ok();
        client.await.unwrap();
        result
    }
    use std::path::PathBuf;
    #[tokio::test]
    async fn rejects_foreign_origins_missing_capabilities_and_oversized_bodies() {
        assert!(
            accepted(
                "Origin: ORIGIN\r\nContent-Type: application/json\r\n",
                "secret",
                "{}"
            )
            .await
        );
        assert!(
            !accepted(
                "Origin: ORIGIN\r\nContent-Type: text/plain\r\n",
                "secret",
                "{}"
            )
            .await
        );
        assert!(
            !accepted(
                "Host: evil.test\r\nOrigin: ORIGIN\r\nContent-Type: application/json\r\n",
                "secret",
                "{}"
            )
            .await
        );
        assert!(
            !accepted(
                "Origin: http://evil.test\r\nContent-Type: application/json\r\n",
                "secret",
                "{}"
            )
            .await
        );
        assert!(!accepted("Content-Type: application/json\r\n", "secret", "{}").await);
        assert!(
            !accepted(
                "Origin: http://evil.test\r\nContent-Type: application/json\r\n",
                "wrong",
                "{}"
            )
            .await
        );
        assert!(
            !accepted(
                "Content-Type: application/json\r\n",
                "secret",
                &"x".repeat(2048)
            )
            .await
        );
    }
}
