use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use rmcp::model::JsonObject;
use serde::Serialize;
use serde_json::Value;

use crate::cache_key::{prefix, DiffId, DiffKey, SCHEMA};
use crate::engine::{DiffFileEntry, Patch};
use crate::error::Failure;
use crate::log::Line;
use crate::page::RESPONSE_CEILING;
use crate::store::{Entry, RequestHeaders};
use crate::tools::{self, Ctx};

const HIT_CACHE_CONTROL: &str = "public, max-age=86400, s-maxage=31536000";

const NOT_A_DIFF_ID: &str = "A diff_id is 64 lowercase hex digits.";

type CtxFactory = Box<dyn Fn(&RequestHeaders) -> Ctx + Send + Sync>;

pub struct Entries {
    ctx: CtxFactory,

    ceiling: usize,
}

impl Entries {
    pub fn over(ctx: impl Fn(&RequestHeaders) -> Ctx + Send + Sync + 'static) -> Self {
        Self {
            ctx: Box::new(ctx),
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
            .route(&format!("/{}{{*asked}}", prefix(SCHEMA)), get(serve))
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

async fn serve(
    State(entries): State<Arc<Entries>>,
    Path(asked): Path<String>,
    request: RequestHeaders,
) -> Response {
    let ctx = (entries.ctx)(&request);
    let line = Line::serving(&route()).about(Some(&arguments(&asked)));

    let body = tools::run(&ctx, line, async |call| {
        let diff_id = DiffId::parse(&asked).ok_or_else(|| Failure::InvalidParams {
            message: NOT_A_DIFF_ID.to_owned(),
        })?;

        Ok(call
            .store()
            .answer_by_id(&diff_id, |entry| entries.body_for(entry))
            .await)
    })
    .await;

    match body {
        Ok(Some(body)) => hit(body),
        Ok(None) | Err(_) => miss(),
    }
}

fn route() -> String {
    format!("/{}", prefix(SCHEMA).trim_end_matches('/'))
}

fn arguments(diff_id: &str) -> JsonObject {
    JsonObject::from_iter([("diff_id".to_owned(), Value::String(diff_id.to_owned()))])
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
            (header::CACHE_CONTROL, HIT_CACHE_CONTROL),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response()
}

fn miss() -> Response {
    (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")]).into_response()
}
