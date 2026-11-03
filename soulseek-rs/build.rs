use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=SOULSEEK_RS_GIT_SHA");
    println!("cargo:rerun-if-env-changed=SOULSEEK_RS_VERSION");
    println!("cargo:rerun-if-changed=build.rs");

    if let Some(git_dir) = git_dir() {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/refs");
    }

    let sha = std::env::var("SOULSEEK_RS_GIT_SHA")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(git_sha)
        .unwrap_or_else(|| "000000000000".to_string());

    assert!(
        !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit()),
        "SOULSEEK_RS_GIT_SHA must be a hexadecimal git SHA, got {sha:?}"
    );

    println!("cargo:rustc-env=SOULSEEK_RS_GIT_SHA={sha}");
}

fn git_dir() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
