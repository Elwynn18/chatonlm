//! B1 : une panique du relais n'écrit que son emplacement, jamais son message
//! (qui peut recopier un titre de résultat ou une requête). L'essai se relance
//! lui-même dans un processus à part, qui pose le crochet du relais et panique.

use std::process::Command;

const CHILD: &str = "CHATONLM_RELAY_PANIC_CHILD";
const SECRET: &str = "Tom & Jérôme à Paris, requête privée";

#[test]
fn a_panic_writes_its_location_only() {
    if std::env::var_os(CHILD).is_some() {
        relay_server::log::install_panic_hook();
        let text = SECRET.to_owned();
        panic!("byte index 12 is not a char boundary of `{text}`");
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "a_panic_writes_its_location_only", "--nocapture"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "le processus enfant devait paniquer"
    );
    assert!(stderr.contains("<3>panic at "), "{stderr}");
    assert!(stderr.contains("panic.rs:"), "{stderr}");
    assert!(
        !stderr.contains("Jérôme") && !stderr.contains("char boundary"),
        "{stderr}"
    );
}
