use anyhow::Error;
use std::{
    fmt::{Display, Formatter, Result},
    str::FromStr,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fork {
    Pacaya,
    Shasta,
}

impl Fork {
    pub fn next(&self) -> Option<Self> {
        match self {
            Fork::Pacaya => Some(Fork::Shasta),
            Fork::Shasta => None,
        }
    }
}

impl Display for Fork {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "{:?}", self)
    }
}

impl FromStr for Fork {
    type Err = Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pacaya" => Ok(Fork::Pacaya),
            "shasta" => Ok(Fork::Shasta),
            _ => Err(Error::msg(format!("Invalid fork: {}", s))),
        }
    }
}
