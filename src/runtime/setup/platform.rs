use super::{Context, Preferences};
use crate::runtime::models::Manager;
use std::{fs, io, path::Path, process::Command};

pub(super) fn command(command: &mut Command) -> io::Result<()> {
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

pub(super) fn private_directory(path: &Path) -> io::Result<()> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(path)?;
        }
        #[cfg(windows)]
        {
            // Values travel through the environment, never through script text.
            command(Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command",r#"
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
Import-Module (Join-Path $PSHOME 'Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
$path = $env:E2EM_SETUP_ROOT
[IO.Directory]::CreateDirectory($path) | Out-Null
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = New-Object Security.AccessControl.DirectorySecurity
$acl.SetOwner($sid)
$acl.SetAccessRuleProtection($true,$false)
foreach ($owner in @($sid, (New-Object Security.Principal.SecurityIdentifier('S-1-5-18')))) {
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($owner,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
}
Set-Acl -LiteralPath $path -AclObject $acl
"#]).env("E2EM_SETUP_ROOT",path))?;
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::other(
            "The configuration directory must not be a link.",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::getuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "The configuration directory must be owned by you and private.",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other(
                "Configuration reparse points are not supported.",
            ));
        }
        let probe = tempfile::NamedTempFile::new_in(path)?;
        e2em_platform::read_private_file(probe.path())?;
    }
    Ok(())
}

pub(super) fn initialize_model(context: &Context, manager: &Manager) -> io::Result<()> {
    if context.root.join("models.json").exists() {
        return Ok(());
    }
    let backend = context
        .binary
        .parent()
        .ok_or_else(|| io::Error::other("Missing native backend."))?;
    #[cfg(target_os = "linux")]
    let backend = if backend == Path::new("/usr/bin") {
        Path::new("/usr/libexec/e2em")
    } else {
        backend
    };
    manager.initialize(
        backend.join(if cfg!(windows) {
            "e2em-inference.exe"
        } else {
            "e2em-inference"
        }),
        backend.join(if cfg!(windows) {
            "onnxruntime.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        }),
    )
}

fn runtime_args(context: &Context, preferences: Preferences) -> Vec<String> {
    let mut args = vec![
        if cfg!(windows) { "--pipe" } else { "--socket" }.into(),
        context.endpoint.clone(),
        "--grants".into(),
        context
            .root
            .join("grants.json")
            .to_string_lossy()
            .into_owned(),
    ];
    if preferences.offline {
        args.push("--offline".into());
    }
    if preferences.auto_update && !preferences.offline {
        args.push("--auto-update".into());
    } else {
        args.push("--no-auto-update".into());
        args.push("--model-no-update".into());
    }
    args
}

#[cfg(target_os = "linux")]
fn systemd_quote(value: &str) -> io::Result<String> {
    if value.chars().any(|c| c.is_control()) {
        return Err(io::Error::other(
            "Control characters are not supported in runtime paths.",
        ));
    }
    Ok(format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    ))
}

#[cfg(target_os = "macos")]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn start(context: &Context, preferences: Preferences, restart: bool) -> io::Result<()> {
    let args = runtime_args(context, preferences);
    #[cfg(target_os = "linux")]
    {
        let directory = context.home.join(".config/systemd/user");
        fs::create_dir_all(&directory)?;
        let executable = systemd_quote(&context.binary.to_string_lossy())?;
        let arguments = args
            .iter()
            .map(|s| systemd_quote(s))
            .collect::<io::Result<Vec<_>>>()?
            .join(" ");
        let unit = format!(
            "[Unit]\nDescription=Local E2EM runtime\n[Service]\nExecStart={executable} {arguments}\nRuntimeDirectory=e2em\nRuntimeDirectoryMode=0700\nUMask=0077\nNoNewPrivileges=yes\nPrivateTmp=yes\nRestart=on-failure\n[Install]\nWantedBy=default.target\n"
        );
        write_launcher(&directory.join("e2emd.service"), unit.as_bytes())?;
        command(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        command(Command::new("systemctl").args([
            "--user",
            if preferences.start_at_login {
                "enable"
            } else {
                "disable"
            },
            "e2emd.service",
        ]))?;
        command(Command::new("systemctl").args([
            "--user",
            if restart { "restart" } else { "start" },
            "e2emd.service",
        ]))?;
    }
    #[cfg(target_os = "macos")]
    {
        let socket = Path::new(&context.endpoint);
        private_directory(
            socket
                .parent()
                .ok_or_else(|| io::Error::other("Missing runtime directory."))?,
        )?;
        let directory = context.home.join("Library/LaunchAgents");
        fs::create_dir_all(&directory)?;
        let path = directory.join("org.e2em.runtime.plist");
        let mut arguments = vec![context.binary.to_string_lossy().into_owned()];
        arguments.extend(args);
        let arguments = arguments
            .iter()
            .map(|s| format!("<string>{}</string>", xml(s)))
            .collect::<String>();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>org.e2em.runtime</string><key>ProgramArguments</key><array>{arguments}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ProcessType</key><string>Background</string><key>Umask</key><integer>63</integer></dict></plist>"
        );
        write_launcher(&path, plist.as_bytes())?;
        let domain = format!("gui/{}", rustix::process::getuid().as_raw());
        // launchctl's disabled state survives reboot; always restore it when
        // changing the login preference, rather than deleting a live plist.
        let service = format!("{domain}/org.e2em.runtime");
        command(Command::new("launchctl").args(["enable", &service]))?;
        if restart
            && Command::new("launchctl")
                .args(["print", &service])
                .output()?
                .status
                .success()
        {
            command(Command::new("launchctl").args(["bootout", &service]))?;
        }
        if !Command::new("launchctl")
            .args(["print", &service])
            .output()?
            .status
            .success()
        {
            command(
                Command::new("launchctl")
                    .arg("bootstrap")
                    .arg(&domain)
                    .arg(&path),
            )?;
        }
        if !preferences.start_at_login {
            command(Command::new("launchctl").args(["disable", &service]))?;
        }
    }
    #[cfg(windows)]
    {
        // HKCU login startup works for standard users, without administrator
        // rights or Task Scheduler policy exceptions. The launcher hides the
        // console and the daemon's existing supervisor manages runtime updates.
        let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
        let script = format!(
            "$ErrorActionPreference = 'Stop'\n$env:PSModulePath = Join-Path $PSHOME 'Modules'\nStart-Process -FilePath {} -ArgumentList @({}) -WindowStyle Hidden\n",
            quote(&context.binary.to_string_lossy()),
            args.iter()
                .map(|s| quote(&format!("\"{s}\"")))
                .collect::<Vec<_>>()
                .join(",")
        );
        let startup = context.root.join("start-runtime.ps1");
        // Windows PowerShell 5.1 needs a BOM for non-ASCII user/profile paths.
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend_from_slice(script.as_bytes());
        write_launcher(&startup, &bytes)?;
        command(Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-Command",r#"
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if ($env:E2EM_SETUP_LOGIN -eq 'yes') {
    if (-not (Test-Path -LiteralPath $key)) { New-Item -Path $key -Force | Out-Null }
    $value = 'powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "' + $env:E2EM_SETUP_STARTUP + '"'
    New-ItemProperty -Path $key -Name 'E2EM Runtime' -PropertyType String -Value $value -Force | Out-Null
} else { Remove-ItemProperty -Path $key -Name 'E2EM Runtime' -ErrorAction SilentlyContinue }
$binary = $env:E2EM_SETUP_BINARY
if ($env:E2EM_SETUP_RESTART -eq 'yes') {
    Get-CimInstance Win32_Process -Filter "Name='e2emd.exe'" | Where-Object { $_.ExecutablePath -eq $binary -and $_.CommandLine -match '(^|\s)"?--grants"?(\s|$)' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
    Start-Sleep -Seconds 2
}
if (-not (Get-CimInstance Win32_Process -Filter "Name='e2emd.exe'" | Where-Object { $_.ExecutablePath -eq $binary -and $_.CommandLine -match '(^|\s)"?--grants"?(\s|$)' })) {
    & $env:E2EM_SETUP_STARTUP
}
"#]).env("E2EM_SETUP_RESTART",if restart {"yes"} else {"no"}).env("E2EM_SETUP_LOGIN",if preferences.start_at_login {"yes"} else {"no"}).env("E2EM_SETUP_STARTUP",&startup).env("E2EM_SETUP_BINARY",&context.binary))?;
    }
    Ok(())
}

fn write_launcher(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() && !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other(
            "Refusing to replace a linked startup file.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing launcher directory."))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(io::Error::other)?;
    Ok(())
}

pub(super) fn browser(url: &str) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    let mut browser = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut browser = Command::new("open");
    #[cfg(windows)]
    let mut browser = {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    browser
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn systemd_paths_escape_expansion_and_quotes() {
        assert_eq!(
            systemd_quote("/home/a $name/100%/\"x\"").unwrap(),
            "\"/home/a $$name/100%%/\\\"x\\\"\""
        );
        assert!(systemd_quote("/home/injected\nExecStart=evil").is_err());
    }
}
