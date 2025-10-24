use anyhow::Error;
use tokio_util::sync::CancellationToken;

pub struct Node {
    cancel_token: CancellationToken,
}

impl Node {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(cancel_token: CancellationToken) -> Result<Self, Error> {
        Ok(Self { cancel_token })
    }
}
