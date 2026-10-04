//! Small native GUI launcher: the daemon owns the shared setup implementation.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{io, process::Command};

fn main() -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let binary = executable
        .parent()
        .ok_or_else(|| io::Error::other("Missing runtime directory."))?
        .join(format!("e2emd{}", std::env::consts::EXE_SUFFIX));
    let mut command = Command::new(binary);
    command.arg("--setup").args(std::env::args_os().skip(1));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command.spawn()?;
    Ok(())
}
