use std::process::Command;

const FALLBACK_ROZE_GIT_REV: &str = "ec75389b3243228c63dd861517b91c58e15cad68";

fn main() {
    println!("cargo:rerun-if-env-changed=ROZE_GIT_REV");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");

    let revision = std::env::var("ROZE_GIT_REV")
        .ok()
        .filter(|value| valid_revision(value))
        .or_else(git_revision)
        .unwrap_or_else(|| FALLBACK_ROZE_GIT_REV.to_string());
    println!("cargo:rustc-env=ROZE_GIT_REV={revision}");
}

fn git_revision() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?;
    let revision = revision.trim();
    valid_revision(revision).then(|| revision.to_string())
}

fn valid_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
