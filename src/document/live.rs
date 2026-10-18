use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::error::Failure;
use crate::fetch::{self, About as Fetching};

use super::{About, Body};

#[derive(Debug)]
pub struct Live;

impl Live {
    pub fn new() -> Self {
        Self
    }

    pub fn held(&self, url: &str, about: &About<'_>) -> Option<Body> {
        let fresh_for = about.remember_for?;
        remembered(url, fresh_for).map(Body::Shared)
    }

    pub async fn fetch(&self, url: &str, limit: u64, about: &About<'_>) -> Result<Body, Failure> {
        let bytes = fetch::bytes(
            url,
            limit,
            &Fetching {
                registry: about.registry,
                accept: about.accept,
                compressed: about.compressed,
                missing: about.missing,
                too_large: about.too_large,
            },
        )
        .await?;

        match about.remember_for {
            Some(_) => {
                let body: Arc<[u8]> = Arc::from(bytes);
                remember(url, &body);
                Ok(Body::Shared(body))
            }
            None => Ok(Body::Owned(bytes)),
        }
    }
}

fn remembered(url: &str, fresh_for: Duration) -> Option<Arc<[u8]>> {
    let memo = memo().read().ok()?;
    let held = memo.as_ref()?;

    (held.url == url && held.fetched.elapsed() < fresh_for).then(|| Arc::clone(&held.body))
}

fn remember(url: &str, body: &Arc<[u8]>) {
    if let Ok(mut memo) = memo().write() {
        *memo = Some(Held {
            url: url.to_owned(),
            fetched: Instant::now(),
            body: Arc::clone(body),
        });
    }
}

fn memo() -> &'static RwLock<Option<Held>> {
    static MEMO: OnceLock<RwLock<Option<Held>>> = OnceLock::new();
    MEMO.get_or_init(|| RwLock::new(None))
}

struct Held {
    url: String,
    fetched: Instant,
    body: Arc<[u8]>,
}
