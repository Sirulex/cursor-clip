pub mod ipc_client;
pub mod shared;

#[cfg(test)]
mod test_support;

pub fn init_logging() -> Result<(), log::SetLoggerError> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .try_init()
}
