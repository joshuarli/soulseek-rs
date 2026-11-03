//! The version string, with the git SHA embedded at build time.

pub const VERSION: &str = match option_env!("SOULSEEK_RS_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

pub const GIT_SHA: &str = match option_env!("SOULSEEK_RS_GIT_SHA") {
    Some(sha) => sha,
    None => "000000000000",
};

#[must_use]
pub fn line() -> String {
    format!("soulseek-rs {VERSION} (git {GIT_SHA})")
}
