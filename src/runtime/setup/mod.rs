//! The same local, user-owned onboarding flow on every desktop platform.
mod http;
mod platform;
pub use http::serve;

use super::{models::Manager, service, update::Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const PRINCIPAL: &str = "e2em-setup";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub offline: bool,
    pub auto_update: bool,
    pub start_at_login: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Context) {
        let home = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let context = Context {
            home: home.path().to_owned(),
            root: home.path().join("e2em"),
            binary: home.path().join("e2emd"),
            requested_app: None,
            endpoint: home
                .path()
                .join("runtime.sock")
                .to_string_lossy()
                .into_owned(),
            progress: Arc::new(Mutex::new(json!({}))),
        };
        (home, context)
    }
    #[test]
    fn retries_and_reenrolment_preserve_credentials_and_provider_identity() {
        let (_home, context) = fixture();
        context.initialize(Preferences::default()).unwrap();
        let path = context.enrol("my-app", true).unwrap();
        let before = fs::read(&path).unwrap();
        let grants = fs::read(context.root.join("grants.json")).unwrap();
        context
            .initialize(Preferences {
                offline: true,
                auto_update: false,
                start_at_login: false,
            })
            .unwrap();
        context.enrol("my-app", true).unwrap();
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(fs::read(context.root.join("grants.json")).unwrap(), grants);
        assert!(context.enrol("../escape", true).is_err());
        assert!(context.enrol(PRINCIPAL, true).is_err());
    }
    #[test]
    fn rejects_an_existing_unmanaged_installation_and_shared_or_linked_directories() {
        let (home, context) = fixture();
        context.initialize(Preferences::default()).unwrap();
        let mut other = context.clone();
        other.binary = home.path().join("other-e2emd");
        assert!(other.initialize(Preferences::default()).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            let root = home.path().join("shared");
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(platform::private_directory(&root).is_err());
            let linked = home.path().join("linked");
            symlink(&context.root, &linked).unwrap();
            assert!(platform::private_directory(&linked).is_err());
        }
    }
    #[tokio::test]
    async fn a_grant_file_without_a_running_model_is_never_ready() {
        let (_home, context) = fixture();
        context.initialize(Preferences::default()).unwrap();
        let status = context.status().await;
        assert_ne!(status["stage"], "ready");
        assert!(!status.to_string().contains("secret"));
        assert!(!status.to_string().contains("provider"));
    }
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            offline: false,
            auto_update: true,
            start_at_login: true,
        }
    }
}

#[derive(Clone)]
pub struct Context {
    pub home: PathBuf,
    pub root: PathBuf,
    pub binary: PathBuf,
    pub endpoint: String,
    pub(super) requested_app: Option<String>,
    progress: Arc<Mutex<Value>>,
}

pub(super) fn random() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

impl Context {
    pub fn discover(home: Option<PathBuf>) -> io::Result<Self> {
        #[cfg(windows)]
        let home_override = home.is_some();
        let home = home
            .or_else(|| {
                std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                    .map(PathBuf::from)
            })
            .ok_or_else(|| io::Error::other("Your user home directory is unavailable."))?;
        #[cfg(unix)]
        if rustix::process::getuid().is_root() {
            return Err(io::Error::other(
                "Open E2EM Setup as your normal user, without sudo.",
            ));
        }
        let home = fs::canonicalize(home)?;
        #[cfg(windows)]
        let root = if home_override {
            home.join("AppData/Local/E2EM")
        } else {
            PathBuf::from(
                std::env::var_os("LOCALAPPDATA")
                    .ok_or_else(|| io::Error::other("Local application data is unavailable."))?,
            )
            .join("E2EM")
        };
        #[cfg(unix)]
        let root = home.join(".config/e2em");
        #[cfg(target_os = "linux")]
        let endpoint = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            io::Error::other("Sign in to a desktop session before opening E2EM Setup.")
        })?)
        .join("e2em/runtime.sock")
        .to_string_lossy()
        .into_owned();
        #[cfg(target_os = "macos")]
        let endpoint = home
            .join("Library/Caches/e2em/runtime.sock")
            .to_string_lossy()
            .into_owned();
        #[cfg(windows)]
        let endpoint = format!(r"\\.\pipe\e2em-{}", e2em_platform::current_user_sid()?);
        #[cfg(unix)]
        if endpoint.len() >= if cfg!(target_os = "macos") { 104 } else { 108 } {
            return Err(io::Error::other("Your runtime socket path is too long."));
        }
        Ok(Self {
            home,
            root,
            binary: std::env::current_exe()?,
            endpoint,
            requested_app: None,
            progress: Arc::new(Mutex::new(
                json!({"stage":"welcome","message":"Set up private message assessment on this computer."}),
            )),
        })
    }

    pub(super) fn progress(&self, stage: &str, message: &str) {
        *self.progress.lock().expect("setup progress lock") =
            json!({"stage":stage,"message":message});
    }

    pub fn with_app(mut self, app: Option<String>) -> io::Result<Self> {
        if app.as_ref().is_some_and(|name| {
            name.is_empty()
                || name.len() > 64
                || name == PRINCIPAL
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        }) {
            return Err(io::Error::other(
                "Invalid application name requested for setup.",
            ));
        }
        self.requested_app = app;
        Ok(self)
    }

    pub(super) fn screen_url(&self, url: &str) -> String {
        match &self.requested_app {
            Some(name) => format!("{url}?app={name}"),
            None => url.to_owned(),
        }
    }

    pub(super) fn store(&self) -> io::Result<Store> {
        platform::private_directory(&self.root)?;
        // Store validates the private directory before any write. Its parent
        // normally accepts only private roots; this root is already validated.
        Store::open(&self.root.join("setup"))
    }

    pub(super) fn lock(&self) -> io::Result<File> {
        let store = self.store()?;
        let path = store.root.join("session.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        fs2::FileExt::try_lock_exclusive(&file)?;
        Ok(file)
    }

    fn config_store(&self) -> io::Result<Store> {
        self.store()?;
        Ok(Store {
            root: self.root.clone(),
        })
    }

    fn initialize(&self, preferences: Preferences) -> io::Result<()> {
        let store = self.config_store()?;
        let grants_path = self.root.join("grants.json");
        let marker_path = self.root.join("installation.json");
        if !grants_path.exists() && !marker_path.exists() {
            let mut marker = json!({"version":1,"packaged_binary":self.binary});
            if !cfg!(target_os = "linux") {
                marker["platform"] = json!(std::env::consts::OS);
            }
            store.write("installation.json", &marker)?;
            store.write(
                "grants.json",
                &json!({"provider":format!("project-{}",random()?),"grants":[]}),
            )?;
        } else {
            // Resume our own interrupted setup, but never replace an unknown
            // installation or rotate any existing application credentials.
            let marker: Value = store.read("installation.json")?;
            if marker["version"] != 1 || marker["packaged_binary"].as_str() != self.binary.to_str()
            {
                return Err(io::Error::other(
                    "This user already has a different E2EM installation. Open its setup tool to continue.",
                ));
            }
            if !grants_path.exists() {
                store.write(
                    "grants.json",
                    &json!({"provider":format!("project-{}",random()?),"grants":[]}),
                )?;
            }
        }
        service::read_grants(&grants_path)?;
        self.enrol(PRINCIPAL, false)?;
        store.write("onboarding.json", &preferences)?;
        Ok(())
    }

    pub(super) fn enrol(&self, principal: &str, credential: bool) -> io::Result<PathBuf> {
        if principal.is_empty()
            || principal.len() > 64
            || !principal
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || (credential && principal == PRINCIPAL)
        {
            return Err(io::Error::other(
                "Use an application name of 1–64 letters, numbers, hyphens, or underscores.",
            ));
        }
        let store = self.config_store()?;
        service::read_grants(&self.root.join("grants.json"))?;
        let mut registry: Value = store.read("grants.json")?;
        let provider = registry["provider"]
            .as_str()
            .ok_or_else(|| io::Error::other("Invalid provider."))?
            .to_owned();
        let grants = registry["grants"]
            .as_array_mut()
            .ok_or_else(|| io::Error::other("Invalid grants."))?;
        let grant = if let Some(grant) = grants.iter().find(|g| g["principal"] == principal) {
            grant.clone()
        } else {
            if grants.len() >= 64 {
                return Err(io::Error::other("The application limit has been reached."));
            }
            let mut grant = json!({"principal":principal,"secret":random()?});
            #[cfg(unix)]
            {
                grant["uid"] = json!(rustix::process::getuid().as_raw());
            }
            #[cfg(windows)]
            {
                grant["sid"] = json!(e2em_platform::current_user_sid()?);
            }
            grants.push(grant.clone());
            store.write("grants.json", &registry)?;
            grant
        };
        let path = if cfg!(windows) {
            self.root.join(format!("app-{principal}.json"))
        } else {
            self.root.join("apps").join(format!("{principal}.json"))
        };
        if credential {
            let mut value = json!({"kind":"project","principal":principal,"secret":grant["secret"],"provider":provider});
            value["socket_path"] = json!(self.endpoint);
            let app_store = if cfg!(windows) {
                store
            } else {
                Store::open(&self.root.join("apps"))?
            };
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| io::Error::other("Invalid credential path."))?;
            if path.exists() {
                let existing: Value = app_store.read(name)?;
                if existing != value {
                    return Err(io::Error::other(
                        "This app already has different credentials. Use its existing enrolment.",
                    ));
                }
            } else {
                app_store.write(name, &value)?;
            }
        }
        Ok(path)
    }

    pub async fn complete(
        &self,
        mut preferences: Preferences,
        source: Option<String>,
    ) -> io::Result<()> {
        let _lock = self.lock()?;
        if preferences.offline {
            preferences.auto_update = false;
        }
        self.perform(preferences, source).await
    }

    pub(super) async fn perform(
        &self,
        preferences: Preferences,
        source: Option<String>,
    ) -> io::Result<()> {
        let context = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let previous = context.config_store()?.read::<Preferences>("onboarding.json").ok();
            let restart = previous.map_or_else(||context.root.join("installation.json").exists(), |old| old != preferences);
            context.progress("configuring", "Preparing your private runtime…");
            context.initialize(preferences)?;
            context.progress("model", "Installing and verifying Gandalf. This can take a few minutes; keep this window open.");
            let manager = Manager::new(context.root.join("models.json"));
            let progress = |done:u64,total:u64| context.progress("model", &format!("Gandalf: {:.1} of {:.1} MB ({}%). Installing and verifying the model…",done as f64/1_000_000.0,total as f64/1_000_000.0,done.saturating_mul(100)/total.max(1)));
            if let Some(source) = source {
                if preferences.offline && !std::path::Path::new(&source).is_dir() {
                    return Err(io::Error::other("Offline setup needs a local model package."));
                }
                platform::initialize_model(&context, &manager)?;
                if manager.config()?.models.is_empty() { manager.install_progress(&source, "gandalf", preferences.auto_update, &progress)?; }
            } else { manager.ensure_default_progress(preferences.offline, preferences.auto_update, &progress)?; }
            manager.set_default_auto_update(preferences.auto_update && !preferences.offline)?;
            context.progress("starting", "Starting E2EM and configuring startup at login…");
            platform::start(&context, preferences, restart)?;
            Ok::<(),io::Error>(())
        }).await.map_err(io::Error::other)?;
        if let Err(error) = result {
            self.progress("error", &format!("Setup could not finish: {error}. Fix the problem and choose Try again; completed steps and app credentials are kept."));
            return Err(error);
        }
        self.progress(
            "checking",
            "Checking the runtime connection and running a local model assessment…",
        );
        let result = self.wait_ready().await;
        match result {
            Ok(()) => self.progress("ready", "E2EM is ready. Your messages are assessed privately on this computer."),
            Err(ref error) => self.progress("error", &format!("E2EM could not pass its readiness check: {error}. Choose Try again to resume setup.")),
        }
        result
    }

    async fn wait_ready(&self) -> io::Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            match tokio::time::timeout(Duration::from_secs(35), self.probe(true)).await {
                Ok(Ok(_)) => return Ok(()),
                result if tokio::time::Instant::now() >= deadline => {
                    return Err(io::Error::other(format!(
                        "The runtime did not become ready ({result:?}). Reopen E2EM Setup after checking your service manager."
                    )));
                }
                _ => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
    }

    pub async fn status(&self) -> Value {
        if !self.root.join("grants.json").exists() {
            return json!({"stage":"welcome","message":"Set up private message assessment on this computer."});
        }
        let model = Manager::new(self.root.join("models.json")).status();
        match tokio::time::timeout(Duration::from_secs(3), self.probe(false)).await {
            Ok(Ok(capabilities)) if model.is_ok() => {
                json!({"stage":"ready","message":"E2EM is running and your model is installed.","model":capabilities["model"],"preferences":self.config_store().and_then(|s|s.read::<Preferences>("onboarding.json")).unwrap_or_default()})
            }
            _ => {
                json!({"stage":"stopped","message":"E2EM needs setup or a restart. Choose Set up E2EM to continue.","preferences":self.config_store().and_then(|s|s.read::<Preferences>("onboarding.json")).unwrap_or_default()})
            }
        }
    }

    async fn probe(&self, assess: bool) -> io::Result<Value> {
        let grants = service::read_grants(&self.root.join("grants.json"))?;
        let grant = grants
            .grants
            .iter()
            .find(|g| g.principal == PRINCIPAL)
            .ok_or_else(|| io::Error::other("Setup connection is not configured."))?;
        #[cfg(unix)]
        let mut stream = tokio::net::UnixStream::connect(&self.endpoint).await?;
        #[cfg(windows)]
        let mut stream =
            tokio::net::windows::named_pipe::ClientOptions::new().open(&self.endpoint)?;
        let nonce = random()?;
        service::write_frame(&mut stream, &json!({"principal":PRINCIPAL,"nonce":nonce})).await?;
        let challenge: Value = serde_json::from_slice(&service::read_frame(&mut stream).await?)
            .map_err(io::Error::other)?;
        let server = challenge["nonce"]
            .as_str()
            .filter(|s| s.len() == 64)
            .ok_or_else(|| io::Error::other("Invalid runtime challenge."))?;
        let expected = service::proof(
            &grant.secret,
            "server",
            &grants.provider,
            PRINCIPAL,
            &nonce,
            server,
        );
        // Compare the fixed-length proof without revealing partial matches.
        let actual = challenge["proof"].as_str().unwrap_or("");
        if challenge["provider"] != grants.provider
            || actual.len() != expected.len()
            || actual
                .bytes()
                .zip(expected.bytes())
                .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                != 0
        {
            return Err(io::Error::other("The runtime could not authenticate."));
        }
        service::write_frame(&mut stream, &json!({"proof":service::proof(&grant.secret,"client",&grants.provider,PRINCIPAL,&nonce,server)})).await?;
        let authenticated: Value = serde_json::from_slice(&service::read_frame(&mut stream).await?)
            .map_err(io::Error::other)?;
        if authenticated["authenticated"] != true || authenticated["provider"] != grants.provider {
            return Err(io::Error::other("Authentication was rejected."));
        }
        service::write_frame(&mut stream, &json!({"call_id":"setup-status","api_version":"0.1","operation":{"op":"capabilities"}})).await?;
        let reply: Value = serde_json::from_slice(&service::read_frame(&mut stream).await?)
            .map_err(io::Error::other)?;
        let capabilities = reply["reply"]["capabilities"].clone();
        if reply["call_id"] != "setup-status"
            || capabilities["model"].as_str().is_none_or(|s| s == "none")
        {
            return Err(io::Error::other("The model is unavailable."));
        }
        if assess {
            let request = super::presets::request(
                "Hello there!",
                super::presets::CheckOptions {
                    policies: Some(vec!["abuse.threat".into()]),
                    deadline_ms: Some(30000),
                    ..Default::default()
                },
            );
            service::write_frame(&mut stream, &json!({"call_id":"setup-check","api_version":"0.1","operation":{"op":"assess","request":request}})).await?;
            let reply: Value = serde_json::from_slice(&service::read_frame(&mut stream).await?)
                .map_err(io::Error::other)?;
            if reply["call_id"] != "setup-check"
                || reply["reply"]["assessment"]["status"] != "assessed"
            {
                return Err(io::Error::other(
                    "The model could not assess the setup test message.",
                ));
            }
        }
        Ok(capabilities)
    }
}
