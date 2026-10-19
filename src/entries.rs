use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::cache_key::{DiffId, DiffKey};
use crate::engine::{DiffFileEntry, Patch};
use crate::page::RESPONSE_CEILING;
use crate::store::{DiffStore, Entry};

type StoreFactory = Box<dyn Fn() -> DiffStore + Send + Sync>;

pub struct Entries {
    store: StoreFactory,

    ceiling: usize,
}

impl Entries {
    pub fn over(store: impl Fn() -> DiffStore + Send + Sync + 'static) -> Self {
        Self {
            store: Box::new(store),
            ceiling: RESPONSE_CEILING,
        }
    }

    pub fn capping_responses_at(self, bytes: usize) -> Self {
        Self {
            ceiling: bytes,
            ..self
        }
    }

    pub fn routes(self) -> Router {
        Router::new()
            .route("/diffs/v1/{diff_id}", get(serve))
            .with_state(Arc::new(self))
            .layer(axum::middleware::map_response(readable_from_any_origin))
    }

    fn body_for(&self, entry: Entry) -> Option<Vec<u8>> {
        let mut served = Served::from(entry);

        let whole = serde_json::to_vec(&served).ok()?;
        if whole.len() <= self.ceiling {
            return Some(whole);
        }

        served.patches.clear();
        let tree_alone = serde_json::to_vec(&served).ok()?;
        (tree_alone.len() <= self.ceiling).then_some(tree_alone)
    }
}

#[derive(Serialize)]
struct Served {
    #[serde(flatten)]
    key: DiffKey,

    tree: DiffFileEntry,

    patches: BTreeMap<String, Patch>,
}

impl From<Entry> for Served {
    fn from(entry: Entry) -> Self {
        Self {
            key: entry.key,
            tree: entry.tree,
            patches: entry.patches,
        }
    }
}

async fn serve(State(entries): State<Arc<Entries>>, Path(diff_id): Path<String>) -> Response {
    let Some(diff_id) = DiffId::parse(&diff_id) else {
        return miss();
    };

    let entry = (entries.store)().get_by_id(&diff_id).await;

    match entry.and_then(|entry| entries.body_for(entry)) {
        Some(body) => hit(body),
        None => miss(),
    }
}

async fn readable_from_any_origin(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

fn hit(body: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        body,
    )
        .into_response()
}

fn miss() -> Response {
    (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")]).into_response()
}
