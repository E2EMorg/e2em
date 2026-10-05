//! Offline reference chat with an application-owned send boundary.
use clap::Parser;
use e2em_runtime::runtime::{integration::RevisionGuard, scheduler::Scheduler, *};
use std::io::{self, Write};
#[derive(Parser)]
#[command(about = "Offline chat assessment with default presets and optional context")]
struct Args {
    #[arg(long = "policy", value_name = "CATEGORY", conflicts_with_all = ["policy_file", "legacy_policy_file"])]
    policies: Vec<String>,
    #[arg(long = "custom", value_name = "TEXT", conflicts_with_all = ["policy_file", "legacy_policy_file"])]
    custom_policies: Vec<String>,
    #[arg(long, value_name = "PRIOR_MESSAGE")]
    context: Vec<String>,
    #[arg(long, conflicts_with_all = ["policy_file", "legacy_policy_file"])]
    no_presets: bool,
    #[arg(long, conflicts_with = "legacy_policy_file")]
    policy_file: Option<std::path::PathBuf>,
    legacy_policy_file: Option<std::path::PathBuf>,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let scheduler = Scheduler::default();
    let policy: Option<Policy> = args
        .policy_file
        .or(args.legacy_policy_file)
        .map(|path| -> Result<Policy, Box<dyn std::error::Error>> {
            let policy = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            validate_policy(&policy)?;
            Ok(policy)
        })
        .transpose()?;
    let options = presets::CheckOptions {
        context: args.context,
        policies: if args.no_presets || !args.policies.is_empty() {
            Some(args.policies)
        } else {
            None
        },
        custom_policies: args.custom_policies,
        ..Default::default()
    };
    let mut revision = 0;
    println!("E2EM reference chat — offline; messages stay local.");
    loop {
        print!("Chat message (empty to exit): ");
        io::stdout().flush()?;
        let mut text = String::new();
        io::stdin().read_line(&mut text)?;
        if text.trim().is_empty() {
            return Ok(());
        }
        revision += 1;
        let mut request = presets::request(&text, options.clone());
        request.message.id = "draft".into();
        request.message.revision = revision.to_string();
        if let Some(policy) = &policy {
            request.policy = Some(policy.clone());
        }
        let guard = RevisionGuard::new(request.clone());
        let result = scheduler
            .submit("reference-chat", request.clone())?
            .wait()?;
        for finding in &result.findings {
            if let Some(score) = finding.score {
                println!(
                    "Category {}: score {score}",
                    finding.category.as_deref().unwrap_or(&finding.rule_id)
                );
            }
        }
        if !result.coverage.unevaluated_rules.is_empty() {
            println!(
                "Unevaluated rules: {} ({})",
                result
                    .coverage
                    .unevaluated_rules
                    .iter()
                    .map(|id| {
                        request
                            .policy
                            .as_ref()
                            .and_then(|policy| policy.rules.iter().find(|rule| &rule.id == id))
                            .and_then(|rule| rule.category.as_deref())
                            .unwrap_or(id)
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                result.reason_codes.join(", ")
            );
        }
        if result.action == Action::Warn {
            println!("Warning: this chat message matched your policy.");
        }
        if guard.can_continue(&result, &request, false) {
            println!("Message accepted by the local reference chat.");
        } else {
            println!("Message held. Edit or retry the draft.");
        }
    }
}
