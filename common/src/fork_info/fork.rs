use std::fmt::{Display, Formatter, Result as FmtResult};
use strum::{EnumIter, IntoEnumIterator};

#[derive(Clone, Debug, PartialEq, Eq, EnumIter)]
pub enum Fork {
    Pacaya,
    Shasta,
    Permissionless,
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
