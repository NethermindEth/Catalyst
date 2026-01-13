use anyhow::Error;

pub trait ConfigTrait: Clone + std::fmt::Display {
    fn read_env_variables() -> Result<Self, Error>
    where
        Self: Sized;
}
