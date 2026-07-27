pub type Config = roze_config::ServiceConfig;

pub fn load(path: impl AsRef<std::path::Path>) -> Result<Config, config::ConfigError> {
    roze_config::load_service(path)
}
