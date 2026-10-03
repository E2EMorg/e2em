//! Canonical GitHub feed, bounded downloads and mandatory SHA-256 verification.
use super::{Activity, Candidate, Channel, Config, MAX_BINARY, Store};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const API: &str = "https://api.github.com/repos/E2EMorg/e2em/releases?per_page=100";
const RELEASES: &str = "https://github.com/E2EMorg/e2em/releases/download/";
const USER_AGENT: &str = concat!("e2em-updater/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize)]
pub struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
}
pub struct Selected {
    version: String,
    binary: Asset,
    checksums: Asset,
}

pub fn target() -> io::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        _ => Err(io::Error::other(
            "no automatic update build for this platform",
        )),
    }
}
fn select(
    releases: Vec<Release>,
    current: &str,
    channel: Channel,
    target: &str,
) -> io::Result<Option<Selected>> {
    let current = semver::Version::parse(current).map_err(io::Error::other)?;
    let newest = releases
        .into_iter()
        .filter(|r| !r.draft && (channel == Channel::Preview || !r.prerelease))
        .filter_map(|r| {
            let version = semver::Version::parse(r.tag_name.strip_prefix('v')?).ok()?;
            if r.tag_name != format!("v{version}")
                || !version.build.is_empty()
                || (channel == Channel::Stable && !version.pre.is_empty())
                || version <= current
            {
                return None;
            }
            Some((version, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((version, release)) = newest else {
        return Ok(None);
    };
    let name = format!(
        "e2em-update-{version}-{target}{}",
        if target.contains("windows") {
            ".exe"
        } else {
            ".bin"
        }
    );
    let take = |name: &str| -> io::Result<Asset> {
        let mut assets = release.assets.iter().filter(|a| a.name == name);
        let asset = assets.next().ok_or_else(|| {
            io::Error::other(format!("release {} has no {name}", release.tag_name))
        })?;
        if assets.next().is_some()
            || asset.size == 0
            || asset.size > MAX_BINARY
            || !asset
                .browser_download_url
                .strip_suffix(&format!("/releases/download/{}/{name}", release.tag_name))
                .is_some_and(|repo| {
                    repo.eq_ignore_ascii_case(RELEASES.trim_end_matches("/releases/download/"))
                })
        {
            return Err(io::Error::other("invalid or duplicate release asset"));
        }
        Ok(Asset {
            name: asset.name.clone(),
            size: asset.size,
            browser_download_url: asset.browser_download_url.clone(),
        })
    };
    Ok(Some(Selected {
        version: version.to_string(),
        binary: take(&name)?,
        checksums: take("SHA256SUMS")?,
    }))
}
fn request(agent: &ureq::Agent, url: &str) -> io::Result<ureq::Response> {
    agent
        .get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(io::Error::other)
}
fn bounded(mut reader: impl Read, max: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.by_ref().take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::other("oversized update response"));
    }
    Ok(bytes)
}
fn checksum(bytes: &[u8], name: &str) -> io::Result<String> {
    let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
    let mut found = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let hash = fields.next().unwrap_or("");
        let file = fields.next().unwrap_or("").trim_start_matches('*');
        if file == name {
            if found.is_some()
                || fields.next().is_some()
                || hash.len() != 64
                || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(io::Error::other("invalid or duplicate update checksum"));
            }
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.ok_or_else(|| io::Error::other("missing update checksum"))
}
pub fn check_and_stage(
    config: &Config,
    store: &Store,
    activity: &Arc<Activity>,
    stop: &AtomicBool,
) -> io::Result<Option<Candidate>> {
    let idle = || {
        if stop.load(Ordering::Acquire) || !activity.idle_for(config.idle) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "foreground work resumed; update deferred",
            ))
        } else {
            Ok(())
        }
    };
    idle()?;
    let agent = ureq::AgentBuilder::new()
        .https_only(true)
        .timeout(Duration::from_secs(60))
        .timeout_connect(Duration::from_secs(10))
        .redirects(5)
        .build();
    let releases: Vec<Release> = serde_json::from_slice(&bounded(
        request(&agent, API)?.into_reader(),
        4 * 1024 * 1024,
    )?)
    .map_err(io::Error::other)?;
    let Some(selected) = select(
        releases,
        env!("CARGO_PKG_VERSION"),
        config.channel,
        target()?,
    )?
    else {
        return Ok(None);
    };
    let state = store.state()?;
    if state
        .rejected
        .as_ref()
        .and_then(|v| semver::Version::parse(v).ok())
        .is_some_and(|v| {
            semver::Version::parse(&selected.version).is_ok_and(|selected| selected <= v)
        })
    {
        return Err(io::Error::other(
            "latest release was rolled back; waiting for a newer release",
        ));
    }
    idle()?;
    let sums = bounded(
        request(&agent, &selected.checksums.browser_download_url)?.into_reader(),
        65_536,
    )?;
    if sums.len() as u64 != selected.checksums.size {
        return Err(io::Error::other("checksum manifest size mismatch"));
    }
    let candidate = Candidate {
        version: selected.version,
        sha256: checksum(&sums, &selected.binary.name)?,
        size: selected.binary.size,
    };
    if store.verify(&candidate).is_ok() {
        return Ok(Some(candidate));
    }
    idle()?;
    let mut reader = request(&agent, &selected.binary.browser_download_url)?.into_reader();
    let mut tmp = tempfile::Builder::new()
        .prefix(".e2em-update-")
        .tempfile_in(&store.root)?;
    transfer(&mut reader, tmp.as_file_mut(), &candidate, idle)?;
    tmp.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    tmp.persist(store.binary(&candidate)?)
        .map_err(io::Error::other)?;
    Ok(Some(candidate))
}

fn transfer(
    mut reader: impl Read,
    mut output: impl Write,
    candidate: &Candidate,
    idle: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buf = [0; 65_536];
    loop {
        idle()?;
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > candidate.size {
            return Err(io::Error::other("update download exceeds advertised size"));
        }
        output.write_all(&buf[..n])?;
        hash.update(&buf[..n]);
    }
    if size != candidate.size || format!("{:x}", hash.finalize()) != candidate.sha256 {
        return Err(io::Error::other(
            "update download checksum or size mismatch",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(tag: &str, prerelease: bool, draft: bool) -> Release {
        let name = format!("e2em-update-{}-x86_64-unknown-linux-musl.bin", &tag[1..]);
        let asset = |name: String| Asset {
            browser_download_url: format!("{RELEASES}{tag}/{name}"),
            name,
            size: 123,
        };
        Release {
            tag_name: tag.into(),
            draft,
            prerelease,
            assets: vec![asset(name), asset("SHA256SUMS".into())],
        }
    }
    #[test]
    fn channels_semver_and_no_downgrades() {
        let releases = || {
            vec![
                release("v0.1.9", false, false),
                release("v0.1.10", true, false),
                release("v1.0.0", false, true),
            ]
        };
        assert_eq!(
            select(
                releases(),
                "0.1.1",
                Channel::Stable,
                "x86_64-unknown-linux-musl"
            )
            .unwrap()
            .unwrap()
            .version,
            "0.1.9"
        );
        assert_eq!(
            select(
                releases(),
                "0.1.1",
                Channel::Preview,
                "x86_64-unknown-linux-musl"
            )
            .unwrap()
            .unwrap()
            .version,
            "0.1.10"
        );
        assert!(
            select(
                releases(),
                "1.0.0",
                Channel::Preview,
                "x86_64-unknown-linux-musl"
            )
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn rejects_missing_target_and_external_downloads() {
        assert!(
            select(
                vec![release("v0.2.0", false, false)],
                "0.1.1",
                Channel::Preview,
                "aarch64-apple-darwin"
            )
            .is_err()
        );
        let mut release = release("v0.2.0", false, false);
        release.assets[0].browser_download_url = "https://example.test/malware".into();
        assert!(
            select(
                vec![release],
                "0.1.1",
                Channel::Preview,
                "x86_64-unknown-linux-musl"
            )
            .is_err()
        );
    }
    #[test]
    fn canonical_repository_case_does_not_change_publisher_identity() {
        let mut release = release("v0.2.0", false, false);
        for asset in &mut release.assets {
            asset.browser_download_url = asset.browser_download_url.replace("E2EMorg", "e2emorg");
        }
        assert!(
            select(
                vec![release],
                "0.1.1",
                Channel::Preview,
                "x86_64-unknown-linux-musl"
            )
            .unwrap()
            .is_some()
        );
    }
    #[test]
    fn checksums_are_required_unique_and_valid() {
        let hash = "a".repeat(64);
        let line = format!("{hash}  runtime.bin\n");
        assert_eq!(checksum(line.as_bytes(), "runtime.bin").unwrap(), hash);
        assert!(checksum(line.repeat(2).as_bytes(), "runtime.bin").is_err());
        assert!(checksum(b"bad runtime.bin", "runtime.bin").is_err());
        assert!(checksum(line.as_bytes(), "other.bin").is_err());
        assert!(bounded(&b"12345"[..], 4).is_err());
    }
    #[test]
    fn transfer_rejects_truncation_corruption_and_oversize() {
        let payload = b"runtime executable";
        let candidate = Candidate {
            version: "0.2.0".into(),
            size: payload.len() as u64,
            sha256: format!("{:x}", Sha256::digest(payload)),
        };
        let mut output = Vec::new();
        transfer(&payload[..], &mut output, &candidate, || Ok(())).unwrap();
        assert_eq!(output, payload);
        for bytes in [
            &payload[..3],
            &b"corrupt executable"[..],
            &b"runtime executable extra bytes"[..],
        ] {
            assert!(transfer(bytes, Vec::new(), &candidate, || Ok(())).is_err());
        }
    }
    #[test]
    fn resumed_work_interrupts_download_before_next_chunk() {
        use std::cell::Cell;
        let chunks = Cell::new(0);
        let bytes = vec![0; 200_000];
        let candidate = Candidate {
            version: "0.2.0".into(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        let mut output = Vec::new();
        let error = transfer(bytes.as_slice(), &mut output, &candidate, || {
            chunks.set(chunks.get() + 1);
            if chunks.get() > 1 {
                Err(io::Error::from(io::ErrorKind::Interrupted))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(output.len(), 65_536);
    }
}
