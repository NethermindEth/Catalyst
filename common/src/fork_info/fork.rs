use std::fmt::{Display, Formatter, Result as FmtResult};
use std::str::FromStr;
use strum::{EnumIter, IntoEnumIterator};

#[derive(Clone, Debug, PartialEq, Eq, EnumIter)]
pub enum Fork {
    Pacaya,
    Shasta,
    Permissionless,
    Realtime,
}

impl FromStr for Fork {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pacaya" => Ok(Fork::Pacaya),
            "shasta" => Ok(Fork::Shasta),
            "permissionless" => Ok(Fork::Permissionless),
            "realtime" => Ok(Fork::Realtime),
            _ => Err(anyhow::anyhow!("Unknown fork: {}", s)),
        }
    }
}

impl Fork {
    pub fn next(&self) -> Option<Self> {
        Fork::iter().skip_while(|f| f != self).nth(1)
    }
}

impl Display for Fork {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{:?}", self)
    }
}
