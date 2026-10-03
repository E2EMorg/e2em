use e2em_runtime::runtime::*;
use std::{fs, path::Path};
fn main() {
    let output = Path::new("docs/runtime/schemas");
    fs::create_dir_all(output).unwrap();
    macro_rules! schema {
        ($name:literal, $ty:ty) => {
            fs::write(
                output.join($name),
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&schemars::schema_for!($ty)).unwrap()
                ),
            )
            .unwrap();
        };
    }
    schema!("request.json", Request);
    schema!("assessment.json", Assessment);
    schema!("policy.json", Policy);
    schema!("capabilities.json", Capabilities);
    schema!("call.json", Call);
    schema!("response.json", Response);
}
