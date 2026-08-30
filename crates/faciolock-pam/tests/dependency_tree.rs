use std::collections::BTreeSet;
use std::process::Command;

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
