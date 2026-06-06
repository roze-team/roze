pub type Config = roze_core::config::ServiceConfig;

pub fn load(path: impl AsRef<std::path::Path>) -> Result<Config, config::ConfigError> {
    roze_core::config::load(path)
}
