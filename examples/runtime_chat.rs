//! Offline reference chat with an application-owned send boundary.
use e2em_runtime::runtime::{integration::RevisionGuard, scheduler::Scheduler, *};
use std::io::{self, Write};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scheduler = Scheduler::default();
    let policy: Policy = serde_json::from_str(include_str!("chat-policy.json"))?;
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
        let request = Request {
            api_version: API_VERSION.into(),
            request_id: format!("req-{revision}"),
            direction: Direction::Outgoing,
            message: Message {
                id: "draft".into(),
                revision: revision.to_string(),
                speaker: "self".into(),
                text,
            },
            context: vec![],
            language: "en".into(),
            options: Options::default(),
            policy_ref: None,
            policy: Some(policy.clone()),
        };
        let guard = RevisionGuard::new(request.clone());
        let result = scheduler
            .submit("reference-chat", request.clone())?
            .wait()?;
        let confirmed = if result.action == Action::Warn {
            print!(
                "This chat message shares an email address. Type send to continue, or edit to replace the draft: "
            );
            io::stdout().flush()?;
            let mut decision = String::new();
            io::stdin().read_line(&mut decision)?;
            decision.trim() == "send"
        } else {
            false
        };
        if guard.can_continue(&result, &request, confirmed) {
            println!("Message accepted by the local reference chat.");
        } else {
            println!("Message held. Edit or retry the draft.");
        }
    }
}
