//! wiki/04's hard rule, as a test: **`faciolock-pam` depends on exactly `libc` and
//! `faciolock-ipc`.**
//!
//! Everything in that tree is compiled into a process that is root while it runs. The rule
//! is the thing a distro security reviewer checks first, and the failure mode it guards
//! against is not a bad dependency — it is a *reasonable* one, added by someone solving a
//! real problem, which nobody notices because the diff looked like one line of Cargo.toml.
//!
//! wiki/09 calls this a CI assertion. There is no CI in this repository yet, so it lands as
//! a test, which is what CI would run anyway.

use std::collections::BTreeSet;
use std::process::Command;

/// Build dependencies are excluded on purpose (`-e normal`): `build.rs` here has none, and
/// a build-time crate is not loaded into `sudo`. Dev-dependencies likewise — the test
/// harness is not shipped.
#[test]
fn the_pam_module_depends_on_exactly_libc_and_faciolock_ipc() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(&cargo)
        .args(["tree", "-p", "faciolock-pam", "-e", "normal", "--prefix", "none"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = String::from_utf8(out.stdout).expect("utf-8");
    // `--prefix none` prints one `name version` per line, the crate itself included, and
    // repeats a shared dependency with a `(*)` marker.
    let found: BTreeSet<&str> = text
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| !name.is_empty())
        .collect();

    let allowed: BTreeSet<&str> =
        ["faciolock-pam", "faciolock-ipc", "libc"].into_iter().collect();
    assert_eq!(
        found, allowed,
        "\nthe PAM module's dependency tree changed.\ncargo tree said:\n{text}"
    );
}
