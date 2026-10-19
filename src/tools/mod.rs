use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt as _;
use rmcp::model::{
    CallToolResult, ContentBlock, JsonObject, Resource, Tool as Definition, ToolAnnotations,
};
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::archive::{Archive, FileMap};
use crate::cache_key::DiffKey;
use crate::catalogue::Catalogue;
use crate::compute;
use crate::error::{Failure, CALL_DEADLINE};
use crate::handle::DiffHandle;
use crate::log::{Line, Lookup, Sink, Spent};
use crate::registry::{Hit, Registry, Versions};
use crate::search::Search;
use crate::store::{DiffStore, Entry, Memory};

macro_rules! tools {
    ($($module:ident::$tool:ident),+ $(,)?) => {
        $(pub mod $module;)+

        pub fn definitions() -> Vec<Definition> {
            let mut tools = vec![$(definition::<$module::$tool>()),+];
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            tools
        }

        pub fn definition_of(name: &str) -> Option<Definition> {
            match name {
                $(<$module::$tool as Tool>::NAME => Some(definition::<$module::$tool>()),)+
                _ => None,
            }
        }

        async fn dispatch(
            name: &str,
            arguments: Option<JsonObject>,
            call: &Call,
        ) -> Result<CallToolResult, Failure> {
            match name {
                $(<$module::$tool as Tool>::NAME => invoke::<$module::$tool>(arguments, call).await,)+
                unknown => Err(Failure::NoSuchTool {
                    name: unknown.to_owned(),
                }),
            }
        }
    };
}

tools! {
    diff_package_versions::DiffPackageVersions,
    get_diff_tree::GetDiffTree,
    get_file_content::GetFileContent,
    get_file_diff::GetFileDiff,
    list_package_files::ListPackageFiles,
    list_package_versions::ListPackageVersions,
    resolve_archive_url::ResolveArchiveUrl,
    search_packages::SearchPackages,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Ctx {
    archive: Arc<Archive>,
    catalogue: Arc<Catalogue>,
    search: Arc<Search>,
    store: Arc<DiffStore>,
    log: Sink,
    deadline: Duration,
}

impl Ctx {
    pub fn new() -> Self {
        Self {
            archive: Arc::new(Archive::live()),
            catalogue: Arc::new(Catalogue::live()),
            search: Arc::new(Search::live()),
            store: Arc::new(DiffStore::live()),
            log: Sink::default(),
            deadline: CALL_DEADLINE,
        }
    }

    pub fn fixture(fixtures: impl AsRef<Path>) -> Self {
        let fixtures = fixtures.as_ref();
        Self {
            archive: Arc::new(Archive::fixture(fixtures.join("archives"))),
            catalogue: Arc::new(Catalogue::fixture(fixtures.join("versions"))),
            search: Arc::new(Search::fixture(fixtures.join("searches"))),
            store: Arc::new(Memory::new().store()),
            log: Sink::default(),
            deadline: CALL_DEADLINE,
        }
    }

    pub fn logging_to(self, log: Sink) -> Self {
        Self { log, ..self }
    }

    pub fn answering_within(self, deadline: Duration) -> Self {
        Self { deadline, ..self }
    }

    pub fn storing_in(self, store: DiffStore) -> Self {
        Self {
            store: Arc::new(store),
            ..self
        }
    }

    pub fn seams(&self) -> &'static [&'static str] {
        let Self {
            archive: _,
            catalogue: _,
            search: _,
            store: _,
            log: _,
            deadline: _,
        } = self;

        &["archive", "catalogue", "search", "store"]
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct Call {
    ctx: Ctx,

    spent: Spent,

    lookup: Lookup,
}

impl Call {
    pub fn new(ctx: &Ctx) -> Self {
        Self {
            ctx: ctx.clone(),
            spent: Spent::new(),
            lookup: Lookup::default(),
        }
    }

    pub fn store(&self) -> Recorded<'_> {
        Recorded {
            store: &self.ctx.store,
            lookup: &self.lookup,
        }
    }

    pub fn archive(&self) -> Timed<'_, Archive> {
        self.timed(&self.ctx.archive)
    }

    pub fn catalogue(&self) -> Timed<'_, Catalogue> {
        self.timed(&self.ctx.catalogue)
    }

    pub fn search(&self) -> Timed<'_, Search> {
        self.timed(&self.ctx.search)
    }

    pub async fn compute<T: Send + 'static>(
        &self,
        doing: &'static str,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, Failure> {
        compute::off_the_runtime(doing, work).await
    }

    fn timed<'a, S>(&'a self, seam: &'a S) -> Timed<'a, S> {
        Timed {
            seam,
            spent: &self.spent,
        }
    }
}

#[derive(Debug)]
pub struct Timed<'a, S> {
    seam: &'a S,
    spent: &'a Spent,
}

impl<S> Clone for Timed<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for Timed<'_, S> {}

#[derive(Debug, Clone, Copy)]
pub struct Recorded<'a> {
    store: &'a Arc<DiffStore>,
    lookup: &'a Lookup,
}

impl Recorded<'_> {
    pub async fn get(self, key: &DiffKey) -> Option<Entry> {
        let entry = self.store.get(key).await;

        match self.store.is_available() {
            true => self.lookup.looked(entry.is_some()),
            false => self.lookup.found_no_store(),
        }

        entry
    }

    pub fn put(self, entry: Entry) {
        Arc::clone(self.store).put(entry);
    }
}

impl Timed<'_, Archive> {
    pub async fn fetch(
        self,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<FileMap, Failure> {
        self.spent
            .while_fetching(self.seam.fetch(registry, package, version))
            .await
    }
}

impl Timed<'_, Catalogue> {
    pub async fn versions(self, registry: Registry, package: &str) -> Result<Versions, Failure> {
        self.spent
            .while_fetching(self.seam.versions(registry, package))
            .await
    }
}

impl Timed<'_, Search> {
    pub async fn hits(
        self,
        registry: Registry,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Hit>, Failure> {
        self.spent
            .while_fetching(self.seam.hits(registry, query, limit))
            .await
    }
}

pub trait Tool {
    const NAME: &'static str;

    const TITLE: &'static str;

    const DESCRIPTION: &'static str;

    const READ_ONLY: bool;

    const IDEMPOTENT: bool;

    const OPEN_WORLD: bool;

    type Args: DeserializeOwned + JsonSchema + Send + 'static;

    type Output: Serialize + JsonSchema + 'static;

    fn call(args: Self::Args, call: &Call) -> impl Future<Output = Result<Self::Output, Failure>>;

    fn links(_output: &Self::Output) -> Vec<Resource> {
        Vec::new()
    }
}

pub async fn call(
    name: &str,
    arguments: Option<JsonObject>,
    ctx: &Ctx,
) -> Result<CallToolResult, ErrorData> {
    let line = Line::calling(name).about(Some(&with_handle_decoded(arguments.as_ref())));

    let answer = run(ctx, line, async |call| {
        dispatch(name, arguments, call).await
    })
    .await;

    match answer {
        Ok(result) => Ok(result),
        Err(failure) => failure.respond(),
    }
}

fn with_handle_decoded(arguments: Option<&JsonObject>) -> JsonObject {
    let mut arguments = arguments.cloned().unwrap_or_default();

    let inputs = arguments
        .get(HANDLE_ARGUMENT)
        .and_then(Value::as_str)
        .and_then(DiffHandle::inputs_named_in);

    if let Some(inputs) = inputs {
        arguments.remove(HANDLE_ARGUMENT);
        for (name, value) in inputs {
            arguments.entry(name).or_insert(value);
        }
    }

    arguments
}

const HANDLE_ARGUMENT: &str = "handle";

pub async fn run<T>(
    ctx: &Ctx,
    line: Line,
    work: impl AsyncFnOnce(&Call) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let call = Call::new(ctx);
    let started = Instant::now();
    let doing = line.subject.doing();

    let guarded = AssertUnwindSafe(work(&call)).catch_unwind();

    let answer = match tokio::time::timeout(ctx.deadline, guarded).await {
        Ok(Ok(answered)) => answered,
        Ok(Err(_panic)) => Err(Failure::Panicked { doing }),
        Err(_elapsed) => Err(Failure::OutOfTime {
            deadline: ctx.deadline,
        }),
    };
    ctx.log.write(
        &line
            .taking(started.elapsed(), &call.spent)
            .cached(&call.lookup)
            .ending(&answer),
    );

    answer
}

const JSON_SCHEMA_FORMATS: [&str; 19] = [
    "date-time",
    "date",
    "time",
    "duration",
    "email",
    "idn-email",
    "hostname",
    "idn-hostname",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uuid",
    "uri-template",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

fn definition<T: Tool>() -> Definition {
    let mut tool = Definition::new(T::NAME, T::DESCRIPTION, Arc::new(JsonObject::new()))
        .with_title(T::TITLE)
        .with_input_schema::<T::Args>()
        .with_output_schema::<T::Output>()
        .with_annotations(
            ToolAnnotations::default()
                .read_only(T::READ_ONLY)
                .idempotent(T::IDEMPOTENT)
                .open_world(T::OPEN_WORLD),
        );

    tool.input_schema = with_standard_formats_only(&tool.input_schema);
    tool.output_schema = tool
        .output_schema
        .map(|schema| with_standard_formats_only(&schema));
    tool
}

fn with_standard_formats_only(schema: &JsonObject) -> Arc<JsonObject> {
    let mut schema = schema.clone();
    drop_unknown_formats(&mut schema);
    Arc::new(schema)
}

fn drop_unknown_formats(schema: &mut JsonObject) {
    schema.retain(|keyword, value| !(keyword == "format" && is_unknown_format(value)));
    schema.values_mut().for_each(drop_unknown_formats_within);
}

fn drop_unknown_formats_within(value: &mut Value) {
    match value {
        Value::Object(schema) => drop_unknown_formats(schema),
        Value::Array(schemas) => schemas.iter_mut().for_each(drop_unknown_formats_within),
        _ => {}
    }
}

fn is_unknown_format(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|format| !JSON_SCHEMA_FORMATS.contains(&format))
}

async fn invoke<T: Tool>(
    arguments: Option<JsonObject>,
    call: &Call,
) -> Result<CallToolResult, Failure> {
    let args = serde_json::from_value::<T::Args>(arguments.unwrap_or_default().into()).map_err(
        |invalid| Failure::InvalidParams {
            message: invalid.to_string(),
        },
    )?;

    let output = T::call(args, call).await?;
    let links = T::links(&output);

    let mut result = serde_json::to_value(output)
        .map(CallToolResult::structured)
        .map_err(|_| Failure::Internal {
            doing: "answering a tool call",
        })?;

    result
        .content
        .extend(links.into_iter().map(ContentBlock::ResourceLink));

    Ok(result)
}
