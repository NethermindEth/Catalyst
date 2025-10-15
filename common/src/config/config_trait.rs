pub trait ConfigTrait: Clone + std::fmt::Display {
    fn read_env_variables() -> Self;
}
