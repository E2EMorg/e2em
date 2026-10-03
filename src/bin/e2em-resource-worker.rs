//! Diagnostic native CPU allocation worker. Not an inference model or service.
use clap::{Parser, ValueEnum};
use e2em_runtime::{BackendError, PolicyScorer, RulesScorer, runtime::process::serve_worker};
use std::{
    io::{self, Write},
    time::Duration,
};
#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Normal,
    HangStartup,
    HangScore,
    WrongVersion,
    OversizedFrame,
    InvalidScore,
    ExitScore,
}
#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 64)]
    buffer_mib: usize,
    #[arg(long, value_enum, default_value_t = Mode::Normal)]
    mode: Mode,
}
struct Probe {
    buffers: Vec<Vec<u8>>,
    scorer: RulesScorer,
    mode: Mode,
}
impl PolicyScorer for Probe {
    fn score(
        &self,
        message: &str,
        context: Option<&str>,
        policy: &str,
    ) -> Result<f64, BackendError> {
        std::hint::black_box(&self.buffers);
        match self.mode {
            Mode::HangScore => std::thread::sleep(Duration::from_secs(60)),
            Mode::ExitScore => std::process::exit(1),
            Mode::InvalidScore => return Ok(f64::NAN),
            _ => (),
        }
        self.scorer.score(message, context, policy)
    }
}
fn main() -> io::Result<()> {
    let args = Args::parse();
    if !(1..=512).contains(&args.buffer_mib) {
        return Err(io::Error::other("CPU buffers must be 1–512 MiB"));
    }
    match args.mode {
        Mode::HangStartup => std::thread::sleep(Duration::from_secs(60)),
        Mode::WrongVersion => {
            let value = br#"{"kind":"ready","api_version":"unsupported"}"#;
            io::stdout().write_all(&(value.len() as u32).to_be_bytes())?;
            io::stdout().write_all(value)?;
            return Ok(());
        }
        Mode::OversizedFrame => {
            io::stdout().write_all(&131073_u32.to_be_bytes())?;
            return Ok(());
        }
        _ => (),
    }
    let bytes = args.buffer_mib * 1024 * 1024;
    let buffers = (0..4).map(|_| vec![0xa5; bytes / 4]).collect();
    serve_worker(&Probe {
        buffers,
        scorer: RulesScorer::new(),
        mode: args.mode,
    })
}
