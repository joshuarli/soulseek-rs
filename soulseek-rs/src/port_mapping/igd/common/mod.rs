pub mod messages;
pub mod options;
pub mod parsing;

pub use self::options::SearchOptions;

pub fn random_port() -> u16 {
    fastrand::u16(32_768..65_535)
}
