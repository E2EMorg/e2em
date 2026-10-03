//! Private model store, owner-controlled provisioning and one disposable worker.
use super::{
    model::{self, Descriptor, Manifest, Metadata},
    process::NativeProcessScorer,
};
use crate::{BackendError, PolicyScorer, RequestControl};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

pub const GANDALF: &str = "https://huggingface.co/krazyjakee/gandalf";
pub const INITIAL: &str =
    "https://github.com/E2EMorg/e2em/releases/download/v0.1.2/gandalf-model.json";
pub const CHANNEL: &str =
    "https://github.com/E2EMorg/e2em/releases/latest/download/gandalf-model.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub current: PathBuf,
    pub previous: Option<PathBuf>,
    pub source: String,
    pub auto_update: bool,
    pub last_check: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub worker: PathBuf,
    pub library: PathBuf,
    pub default: String,
    pub models: BTreeMap<String, Installed>,
}
#[derive(Clone)]
pub struct Manager {
    pub path: PathBuf,
}
fn private(path: &Path, directory: bool) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if if directory { !m.is_dir() } else { !m.is_file() } {
        return Err(io::Error::other("model store path is not regular"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.uid() != rustix::process::getuid().as_raw() || m.mode() & 0o077 != 0 {
            return Err(io::Error::other("model store must be owned and private"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other("model reparse points are forbidden"));
        }
    }
    Ok(())
}
fn directory(path: &Path) -> io::Result<()> {
    if !path.exists() {
        fs::create_dir(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    private(path, true)
}
fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.len() > 65536 {
        return Err(io::Error::other("invalid model configuration file"));
    }
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}
fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing model store parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), value).map_err(io::Error::other)?;
    temporary.as_file_mut().write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.persist(path).map_err(io::Error::other)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn decode_hex(value: &str) -> io::Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) || !model::hex(value, value.len()) {
        return Err(io::Error::other("invalid signature encoding"));
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(io::Error::other))
        .collect()
}
pub fn verify_descriptor(descriptor: &Descriptor, trusted: bool) -> io::Result<()> {
    descriptor.manifest.validate()?;
    if trusted {
        use ed25519_dalek::{Signature, VerifyingKey};
        let key: [u8; 32] = decode_hex(include_str!("gandalf-public-key.hex").trim())?
            .try_into()
            .map_err(|_| io::Error::other("invalid model trust key"))?;
        let signature = Signature::from_slice(&decode_hex(
            descriptor
                .signature
                .as_deref()
                .ok_or_else(|| io::Error::other("default model release is unsigned"))?,
        )?)
        .map_err(io::Error::other)?;
        let canonical = serde_json::to_vec(
            &serde_json::to_value(&descriptor.manifest).map_err(io::Error::other)?,
        )
        .map_err(io::Error::other)?;
        VerifyingKey::from_bytes(&key)
            .map_err(io::Error::other)?
            .verify_strict(&canonical, &signature)
            .map_err(io::Error::other)?;
        if descriptor.manifest.id != "gandalf" {
            return Err(io::Error::other("default descriptor is not Gandalf"));
        }
    }
    Ok(())
}
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .https_only(true)
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .timeout_connect(Duration::from_secs(10))
        .redirects(5)
        .build()
}
fn endpoint(source: &str, update: bool) -> String {
    if source == "gandalf" || source.trim_end_matches('/') == GANDALF {
        return if update { CHANNEL } else { INITIAL }.into();
    }
    if let Some(repo) = source.strip_prefix("https://huggingface.co/")
        && repo.trim_end_matches('/').split('/').count() == 2
    {
        return format!(
            "{}/resolve/main/e2em-model.json",
            source.trim_end_matches('/')
        );
    }
    source.into()
}
fn descriptor(source: &str, update: bool) -> io::Result<(Descriptor, Option<PathBuf>)> {
    let path = Path::new(source);
    if path.is_dir() {
        return Ok((read(&path.join("model.json"))?, Some(path.to_owned())));
    }
    let url = endpoint(source, update);
    if !url.starts_with("https://") {
        return Err(io::Error::other(
            "model source must be a local package or HTTPS descriptor",
        ));
    }
    let response = agent()
        .get(&url)
        .set("User-Agent", "e2em-models/0.1.2")
        .call()
        .map_err(io::Error::other)?;
    let mut bytes = Vec::new();
    response.into_reader().take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(io::Error::other("oversized model descriptor"));
    }
    Ok((
        serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        None,
    ))
}
impl Manager {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    fn root(&self) -> io::Result<PathBuf> {
        Ok(self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("model config needs a parent"))?
            .join("models"))
    }
    fn lock(&self) -> io::Result<File> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("model config needs a parent"))?;
        private(parent, true)?;
        let root = self.root()?;
        directory(&root)?;
        let path = root.join("manager.lock");
        if path.exists() {
            private(&path, false)?;
        }
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
    pub fn initialize(&self, worker: PathBuf, library: PathBuf) -> io::Result<()> {
        let _lock = self.lock()?;
        if self.path.exists() {
            return Err(io::Error::other("model configuration already exists"));
        }
        if !worker.is_absolute()
            || !worker.is_file()
            || !library.is_absolute()
            || !library.is_file()
        {
            return Err(io::Error::other(
                "provision matching native worker and ONNX library first",
            ));
        }
        write(
            &self.path,
            &Config {
                version: 1,
                worker,
                library,
                default: "gandalf".into(),
                models: BTreeMap::new(),
            },
        )
    }
    /// Migrate an existing user installation on its first start after upgrade.
    /// Backend paths come only from the installed executable/owner setup marker.
    pub fn ensure_default(&self, offline: bool, auto_update: bool) -> io::Result<()> {
        if !self.path.exists() {
            let parent = self
                .path
                .parent()
                .ok_or_else(|| io::Error::other("missing model config parent"))?;
            let marker = parent.join("installation.json");
            let binary = if marker.exists() {
                let value: serde_json::Value = read(&marker)?;
                value["packaged_binary"]
                    .as_str()
                    .map(PathBuf::from)
                    .unwrap_or(std::env::current_exe()?)
            } else {
                std::env::current_exe()?
            };
            if !binary.is_absolute() {
                return Err(io::Error::other("invalid packaged executable path"));
            }
            let backend = binary
                .parent()
                .ok_or_else(|| io::Error::other("missing packaged backend"))?;
            #[cfg(target_os = "linux")]
            let backend = if backend == Path::new("/usr/bin") {
                Path::new("/usr/libexec/e2em")
            } else {
                backend
            };
            let worker = backend.join(if cfg!(windows) {
                "e2em-inference.exe"
            } else {
                "e2em-inference"
            });
            let library = backend.join(if cfg!(windows) {
                "onnxruntime.dll"
            } else if cfg!(target_os = "macos") {
                "libonnxruntime.dylib"
            } else {
                "libonnxruntime.so"
            });
            self.initialize(worker, library)?;
        }
        let config = self.config()?;
        if config.models.contains_key(&config.default) {
            return Ok(());
        }
        if config.default != "gandalf" {
            return Err(io::Error::other(
                "configured default model is not installed",
            ));
        }
        let backend = config
            .worker
            .parent()
            .ok_or_else(|| io::Error::other("missing model backend"))?;
        let bundled = if cfg!(windows) {
            backend.join("models/gandalf")
        } else {
            backend
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| io::Error::other("missing package prefix"))?
                .join("share/e2em/models/gandalf")
        };
        let source = if bundled.is_dir() {
            bundled.to_string_lossy().into_owned()
        } else if offline {
            return Err(io::Error::other(
                "offline startup needs the bundled or previously provisioned model",
            ));
        } else {
            "gandalf".into()
        };
        eprintln!("Provisioning verified Gandalf assets before runtime startup.");
        self.install(&source, "gandalf", auto_update && !offline)?;
        Ok(())
    }
    pub fn config(&self) -> io::Result<Config> {
        private(&self.path, false)?;
        #[cfg(windows)]
        e2em_platform::read_private_file(&self.path)?;
        let config: Config = read(&self.path)?;
        if config.version != 1
            || !model::name(&config.default)
            || config.models.len() > 8
            || config.models.keys().any(|s| !model::name(s))
            || !config.worker.is_absolute()
            || !config.worker.is_file()
            || !config.library.is_absolute()
            || !config.library.is_file()
        {
            return Err(io::Error::other("invalid native model configuration"));
        }
        let root = self.root()?;
        private(&root, true)?;
        let root = fs::canonicalize(root)?;
        for installed in config.models.values() {
            for path in std::iter::once(&installed.current).chain(installed.previous.iter()) {
                if path
                    .parent()
                    .and_then(|parent| fs::canonicalize(parent).ok())
                    .as_ref()
                    != Some(&root)
                {
                    return Err(io::Error::other("model path escapes private store"));
                }
                private(path, true)?;
            }
        }
        Ok(config)
    }
    pub fn install(&self, source: &str, alias: &str, auto_update: bool) -> io::Result<Manifest> {
        self.install_with(source, alias, auto_update, false, &|| Ok(()))
    }
    fn install_with(
        &self,
        source: &str,
        alias: &str,
        auto_update: bool,
        update: bool,
        admission: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<Manifest> {
        if !model::name(alias) {
            return Err(io::Error::other("invalid model alias"));
        }
        let _lock = self.lock()?;
        let mut config = self.config()?;
        if config.models.len() >= 8 && !config.models.contains_key(alias) {
            return Err(io::Error::other("model registration limit reached"));
        }
        admission()?;
        let (candidate, local) = descriptor(source, update)?;
        let trusted = alias == "gandalf";
        verify_descriptor(&candidate, trusted)?;
        if update && let Some(old) = config.models.get(alias) {
            let previous: Descriptor = read(&old.current.join("model.json"))?;
            if semver::Version::parse(&candidate.manifest.version).map_err(io::Error::other)?
                <= semver::Version::parse(&previous.manifest.version).map_err(io::Error::other)?
            {
                config.models.get_mut(alias).unwrap().last_check = super::update::now();
                write(&self.path, &config)?;
                return Ok(previous.manifest);
            }
        }
        let root = self.root()?;
        let package_name = format!(
            "{}-{}-{}",
            candidate.manifest.id,
            candidate.manifest.version,
            &candidate.manifest.files["model.onnx"].sha256[..16]
        );
        let destination = root.join(&package_name);
        if !destination.exists() {
            let stage = root.join(format!(".{package_name}.staging"));
            directory(&stage)?;
            for (name, asset) in &candidate.manifest.files {
                admission()?;
                let final_path = stage.join(name);
                if final_path.exists()
                    && model::digest(&final_path)? == asset.sha256
                    && final_path.metadata()?.len() == asset.bytes
                {
                    continue;
                }
                let part = stage.join(format!("{name}.part"));
                let mut options = OpenOptions::new();
                options.read(true).write(true).create(true).truncate(false);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                if part.exists() {
                    private(&part, false)?;
                }
                let mut output = options.open(&part)?;
                let start = output.metadata()?.len();
                let mut input: Box<dyn Read> = if let Some(local) = &local {
                    let path = local.join(name);
                    let m = fs::symlink_metadata(&path)?;
                    if !m.is_file() || m.len() != asset.bytes {
                        return Err(io::Error::other("invalid offline model asset"));
                    }
                    output.set_len(0)?;
                    Box::new(File::open(path)?)
                } else {
                    let client = agent();
                    let request = client
                        .get(&asset.url)
                        .set("User-Agent", "e2em-models/0.1.2");
                    let response = if start > 0 && start < asset.bytes {
                        request.set("Range", &format!("bytes={start}-")).call()
                    } else {
                        request.call()
                    }
                    .map_err(io::Error::other)?;
                    if response.status() == 206
                        && !response
                            .header("Content-Range")
                            .is_some_and(|value| value.starts_with(&format!("bytes {start}-")))
                    {
                        return Err(io::Error::other("invalid resumed model response"));
                    }
                    if response.status() != 206 || start == 0 || start >= asset.bytes {
                        output.set_len(0)?;
                    }
                    Box::new(response.into_reader())
                };
                use std::io::{Seek, SeekFrom};
                output.seek(SeekFrom::End(0))?;
                let mut size = output.metadata()?.len();
                let mut buffer = [0; 65536];
                loop {
                    admission()?;
                    let count = input.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    size += count as u64;
                    if size > asset.bytes {
                        return Err(io::Error::other("model asset exceeds descriptor size"));
                    }
                    output.write_all(&buffer[..count])?;
                }
                output.sync_all()?;
                if size != asset.bytes || model::digest(&part)? != asset.sha256 {
                    drop(output);
                    fs::remove_file(part)?;
                    return Err(io::Error::other("model download checksum or size mismatch"));
                }
                drop(output);
                fs::rename(part, final_path)?;
            }
            write(&stage.join("model.json"), &candidate)?;
            candidate.manifest.verify(&stage)?;
            admission()?;
            let worker = spawn(&config, &stage, Duration::from_secs(30), None)?;
            let scores = worker
                .score_many(
                    "Thank you for your help.",
                    None,
                    &["Do not threaten to injure, kill, or otherwise harm a person.".into()],
                )
                .map_err(io::Error::other)?;
            if scores.len() != 1 {
                return Err(io::Error::other(
                    "candidate model failed startup smoke check",
                ));
            }
            drop(worker);
            admission()?;
            fs::rename(&stage, &destination)?;
        }
        candidate.manifest.verify(&destination)?;
        let previous = config.models.get(alias).and_then(|old| {
            if old.current == destination {
                old.previous.clone()
            } else {
                Some(old.current.clone())
            }
        });
        config.models.insert(
            alias.into(),
            Installed {
                current: destination,
                previous,
                source: if alias == "gandalf" {
                    "gandalf".into()
                } else {
                    source.into()
                },
                auto_update,
                last_check: super::update::now(),
            },
        );
        admission()?;
        write(&self.path, &config)?;
        Ok(candidate.manifest)
    }
    pub fn set_default(&self, alias: &str) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut config = self.config()?;
        if !config.models.contains_key(alias) {
            return Err(io::Error::other("model is not registered"));
        }
        config.default = alias.into();
        write(&self.path, &config)
    }
    pub fn rollback(&self, alias: &str) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut config = self.config()?;
        let installed = config
            .models
            .get_mut(alias)
            .ok_or_else(|| io::Error::other("unknown model"))?;
        let previous = installed
            .previous
            .take()
            .ok_or_else(|| io::Error::other("no previous model"))?;
        let descriptor: Descriptor = read(&previous.join("model.json"))?;
        verify_descriptor(&descriptor, alias == "gandalf")?;
        descriptor.manifest.verify(&previous)?;
        installed.previous = Some(std::mem::replace(&mut installed.current, previous));
        installed.auto_update = false;
        write(&self.path, &config)
    }
    pub fn check_updates(
        &self,
        force: bool,
        admission: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        for (alias, installed) in self.config()?.models {
            if installed.auto_update
                && (force || super::update::now().saturating_sub(installed.last_check) >= 21600)
            {
                self.install_with(&installed.source, &alias, true, true, admission)?;
            }
        }
        Ok(())
    }
    pub fn status(&self) -> io::Result<serde_json::Value> {
        let config = self.config()?;
        let mut models = Vec::new();
        for (alias, installed) in &config.models {
            let descriptor: Descriptor = read(&installed.current.join("model.json"))?;
            verify_descriptor(&descriptor, alias == "gandalf")?;
            models.push(serde_json::json!({"alias": alias, "identity": descriptor.manifest.identity(), "version": descriptor.manifest.version, "source": installed.source, "auto_update": installed.auto_update, "last_check": installed.last_check, "rollback_available": installed.previous.is_some()}));
        }
        Ok(serde_json::json!({"default":config.default,"models":models}))
    }
}
fn spawn(
    config: &Config,
    path: &Path,
    timeout: Duration,
    control: Option<RequestControl>,
) -> io::Result<NativeProcessScorer> {
    let descriptor: Descriptor = read(&path.join("model.json"))?;
    let args: Vec<OsString> = vec![
        "--model".into(),
        path.as_os_str().into(),
        "--library".into(),
        config.library.as_os_str().into(),
    ];
    let scorer = NativeProcessScorer::spawn_controlled(&config.worker, &args, timeout, control)
        .map_err(io::Error::other)?;
    let metadata = descriptor.manifest.metadata(path)?;
    if scorer.model_version() != metadata.model
        || scorer.tokenizer_version() != metadata.tokenizer
        || scorer.max_tokens() != metadata.max_tokens
        || !scorer.supports_model_categories()
        || !scorer.supports_custom_policies()
    {
        return Err(io::Error::other(
            "native worker metadata differs from model descriptor",
        ));
    }
    Ok(scorer)
}
struct Active {
    identity: String,
    scorer: Option<NativeProcessScorer>,
    metadata: Metadata,
}
pub struct Router {
    manager: Manager,
    default: Metadata,
    active: Mutex<Active>,
}
impl Router {
    pub fn open(manager: Manager) -> io::Result<Self> {
        let config = manager.config()?;
        let installed = config
            .models
            .get(&config.default)
            .ok_or_else(|| io::Error::other("default model has not been installed"))?;
        let descriptor: Descriptor = read(&installed.current.join("model.json"))?;
        verify_descriptor(&descriptor, config.default == "gandalf")?;
        descriptor.manifest.verify(&installed.current)?;
        let metadata = descriptor.manifest.metadata(&installed.current)?;
        Ok(Self {
            manager,
            default: metadata.clone(),
            active: Mutex::new(Active {
                identity: String::new(),
                scorer: None,
                metadata,
            }),
        })
    }
    fn with<T>(
        &self,
        f: impl FnOnce(&NativeProcessScorer) -> Result<T, BackendError>,
    ) -> Result<T, BackendError> {
        let active = self
            .active
            .lock()
            .map_err(|_| BackendError::new("model router failed"))?;
        f(active
            .scorer
            .as_ref()
            .ok_or_else(|| BackendError::new("model worker unavailable"))?)
    }
}
impl PolicyScorer for Router {
    fn begin_request(
        &self,
        model: Option<&str>,
        control: RequestControl,
        needs_model: bool,
    ) -> Result<(), BackendError> {
        let execute = || -> io::Result<()> {
            if control.stopped() {
                return Err(io::Error::other("request expired"));
            }
            let config = self.manager.config()?;
            let alias = model.unwrap_or(&config.default);
            let installed = config
                .models
                .get(alias)
                .ok_or_else(|| io::Error::other("requested model is not registered"))?;
            let descriptor: Descriptor = read(&installed.current.join("model.json"))?;
            verify_descriptor(&descriptor, alias == "gandalf")?;
            let identity = descriptor.manifest.identity();
            let mut active = self
                .active
                .lock()
                .map_err(|_| io::Error::other("model router failed"))?;
            active.metadata = descriptor.manifest.metadata(&installed.current)?;
            if !needs_model {
                active.metadata = descriptor.manifest.metadata(&installed.current)?;
                if let Some(scorer) = &active.scorer {
                    scorer
                        .begin_request(None, control, false)
                        .map_err(io::Error::other)?;
                }
                return Ok(());
            }
            if active.identity != identity
                || active
                    .scorer
                    .as_ref()
                    .is_none_or(|s| s.process_id().is_none())
            {
                active.scorer.take();
                descriptor.manifest.verify(&installed.current)?;
                let timeout = control
                    .deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(30));
                let scorer = spawn(&config, &installed.current, timeout, Some(control.clone()))?;
                active.metadata = descriptor.manifest.metadata(&installed.current)?;
                active.scorer = Some(scorer);
                active.identity = identity;
            }
            active
                .scorer
                .as_ref()
                .unwrap()
                .begin_request(None, control, true)
                .map_err(io::Error::other)
        };
        execute().map_err(|_| BackendError::new("selected model unavailable"))
    }
    fn supports_model_categories(&self) -> bool {
        true
    }
    fn supports_custom_policies(&self) -> bool {
        true
    }
    fn capabilities_model_version(&self) -> String {
        self.default.model.clone()
    }
    fn model_version(&self) -> String {
        self.active
            .lock()
            .map_or_else(|_| "unavailable".into(), |a| a.metadata.model.clone())
    }
    fn tokenizer_version(&self) -> String {
        self.default.tokenizer.clone()
    }
    fn max_tokens(&self) -> Option<usize> {
        self.default.max_tokens
    }
    fn coverage(&self) -> (bool, bool) {
        self.with(|s| Ok(s.coverage())).unwrap_or((true, true))
    }
    fn action_threshold(&self, policy: &str) -> Option<f64> {
        self.active
            .lock()
            .ok()?
            .metadata
            .thresholds
            .iter()
            .find(|t| t.wording == policy)
            .map(|t| t.action)
    }
    fn review_threshold(&self, policy: &str) -> Option<f64> {
        self.active
            .lock()
            .ok()?
            .metadata
            .thresholds
            .iter()
            .find(|t| t.wording == policy)
            .map(|t| t.review)
    }
    fn score(
        &self,
        message: &str,
        context: Option<&str>,
        policy: &str,
    ) -> Result<f64, BackendError> {
        self.with(|s| s.score(message, context, policy))
    }
    fn score_many(
        &self,
        message: &str,
        context: Option<&str>,
        policies: &[String],
    ) -> Result<Vec<f64>, BackendError> {
        self.with(|s| s.score_many(message, context, policies))
    }
}
