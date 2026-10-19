use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeSet;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::engine;
use crate::error::Failure;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Registry {
    Npm,
    Crates,
    PyPi,
    Go,
    Composer,
}

impl Registry {
    pub const ALL: [Registry; 5] = [
        Registry::Npm,
        Registry::Crates,
        Registry::PyPi,
        Registry::Go,
        Registry::Composer,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Crates => "crates",
            Self::PyPi => "pypi",
            Self::Go => "go",
            Self::Composer => "composer",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|registry| registry.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Crates => "crates.io",
            Self::PyPi => "PyPI",
            Self::Go => "Go",
            Self::Composer => "Packagist",
        }
    }

    pub fn archive(self, package: &str, version: &str) -> Result<ArchiveSource, Failure> {
        match engine::archive_source(self.id(), package, version) {
            Ok(engine::ArchiveSource::Archive { url }) => Ok(ArchiveSource::Archive { url }),
            Ok(engine::ArchiveSource::Listing { url }) => Ok(ArchiveSource::Listing { url }),
            Err(_) => Err(self.unbuildable_archive_url(package)),
        }
    }

    fn unbuildable_archive_url(self, package: &str) -> Failure {
        match self {
            Self::Composer => Failure::NoSuchPackage {
                registry: self.name().to_owned(),
                package: package.to_owned(),
            },
            Self::Npm | Self::Crates | Self::PyPi | Self::Go => Failure::Internal {
                doing: "building an archive URL",
            },
        }
    }

    pub fn name_rule(self) -> &'static str {
        match self {
            Self::Npm => {
                "A scoped name is one package name, `@` and `/` included: \
                          `@types/node`, not `types/node` and not `node`."
            }
            Self::Crates => {
                "A crate name is one segment, and `-` and `_` are different \
                             names: `serde_json` is not `serde-json`."
            }
            Self::PyPi => {
                "The name is spelled as PyPI spells it. No PEP 503 \
                           normalisation is applied, so `Typing.Extensions` keeps its \
                           case and its dot."
            }
            Self::Go => {
                "A module path is spelled in full, its major-version suffix \
                           included: `github.com/go-chi/chi/v5`, not `github.com/go-chi/chi` \
                           and not `chi`. Case is kept: `github.com/Masterminds/semver/v3`."
            }
            Self::Composer => {
                "A package name is a vendor and a package joined by one `/`, in \
                           lower case: `monolog/monolog`, not `monolog` and not \
                           `Monolog/Monolog`."
            }
        }
    }

    pub fn manifest(self) -> &'static str {
        match self {
            Self::Npm => "`package.json`",
            Self::Crates => "`Cargo.toml`",
            Self::PyPi => "`PKG-INFO`, `pyproject.toml` or `setup.py`",
            Self::Go => "`go.mod`",
            Self::Composer => "`composer.json`",
        }
    }

    pub fn runtime(self) -> &'static str {
        match self {
            Self::Npm => "Node.js",
            Self::Crates => "Rust",
            Self::PyPi => "Python",
            Self::Go => "Go",
            Self::Composer => "PHP",
        }
    }

    pub fn versions(self, package: &str) -> VersionSource {
        let escaped = escape(package);
        match self {
            Self::Npm => {
                VersionSource::one_document(format!("https://registry.npmjs.org/{escaped}"))
            }
            Self::Crates => {
                VersionSource::one_document(format!("https://crates.io/api/v1/crates/{escaped}"))
            }
            Self::PyPi => VersionSource::one_document(format!(
                "https://api.deps.dev/v3/systems/pypi/packages/{escaped}"
            )),
            Self::Go => {
                let module = go_module_path(package);
                VersionSource {
                    url: format!("{GO_PROXY}/{module}/@v/list"),
                    current: Some(format!("{GO_PROXY}/{module}/@latest")),
                }
            }
            Self::Composer => VersionSource::one_document(format!(
                "{PACKAGIST_REPOSITORY}/p2/{}.json",
                composer_package_path(package)
            )),
        }
    }

    pub fn read_versions(self, document: &str) -> Option<Versions> {
        let (mut versions, current) = match self {
            Self::Npm => {
                #[derive(Deserialize)]
                struct Document {
                    versions: std::collections::BTreeMap<String, de::IgnoredAny>,
                    time: std::collections::BTreeMap<String, String>,
                    #[serde(default, rename = "dist-tags")]
                    dist_tags: DistTags,
                }
                #[derive(Default, Deserialize)]
                struct DistTags {
                    #[serde(default)]
                    latest: Option<String>,
                }

                let document: Document = serde_json::from_str(document).ok()?;
                let listed = document
                    .versions
                    .into_keys()
                    .map(|version| Version {
                        prerelease: self.is_prerelease(&version),
                        published_at: document.time.get(&version).cloned(),
                        version,
                    })
                    .collect::<Vec<_>>();
                (listed, document.dist_tags.latest)
            }

            Self::Crates => {
                #[derive(Deserialize)]
                struct Document {
                    versions: Vec<Release>,
                    #[serde(rename = "crate")]
                    package: Package,
                }
                #[derive(Deserialize)]
                struct Package {
                    #[serde(default)]
                    default_version: Option<String>,
                }
                #[derive(Deserialize)]
                struct Release {
                    num: String,
                    created_at: Option<String>,
                }

                let document: Document = serde_json::from_str(document).ok()?;
                let listed = document
                    .versions
                    .into_iter()
                    .map(|release| Version {
                        prerelease: self.is_prerelease(&release.num),
                        version: release.num,
                        published_at: release.created_at,
                    })
                    .collect::<Vec<_>>();
                (listed, document.package.default_version)
            }

            Self::PyPi => {
                #[derive(Deserialize)]
                struct Document {
                    versions: Vec<Release>,
                }
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Release {
                    version_key: VersionKey,
                    #[serde(default)]
                    published_at: Option<String>,
                    #[serde(default)]
                    is_default: bool,
                }
                #[derive(Deserialize)]
                struct VersionKey {
                    version: String,
                }

                let document: Document = serde_json::from_str(document).ok()?;
                let mut current = None;
                let listed = document
                    .versions
                    .into_iter()
                    .map(|release| {
                        if release.is_default {
                            current = Some(release.version_key.version.clone());
                        }
                        Version {
                            prerelease: self.is_prerelease(&release.version_key.version),
                            version: release.version_key.version,
                            published_at: release.published_at,
                        }
                    })
                    .collect::<Vec<_>>();
                (listed, current)
            }

            Self::Go => {
                let listed = document
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|version| {
                        version.starts_with('v').then(|| Version {
                            prerelease: self.is_prerelease(version),
                            version: version.to_owned(),
                            published_at: None,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                (listed, None)
            }

            Self::Composer => {
                let listed = packagist_versions(document)?;
                let current = listed
                    .iter()
                    .find(|version| !version.prerelease)
                    .map(|version| version.version.clone());
                (listed, current)
            }
        };

        versions.sort_by(|a, b| {
            b.published_at
                .cmp(&a.published_at)
                .then_with(|| self.undated_order(&b.version, &a.version))
        });
        Some(Versions {
            all: versions,
            current,
        })
    }

    pub fn read_current(self, document: &str) -> Option<Version> {
        match self {
            Self::Npm | Self::Crates | Self::PyPi | Self::Composer => None,
            Self::Go => {
                #[derive(Deserialize)]
                #[serde(rename_all = "PascalCase")]
                struct Latest {
                    version: String,
                    time: Option<String>,
                }

                let latest: Latest = serde_json::from_str(document).ok()?;
                Some(Version {
                    prerelease: self.is_prerelease(&latest.version),
                    version: latest.version,
                    published_at: latest.time,
                })
            }
        }
    }

    pub fn search(self, query: &str, limit: u32) -> SearchSource {
        let escaped = escape(query);
        match self {
            Self::Npm => {
                let size = limit.min(250);
                SearchSource::asking(format!(
                    "https://registry.npmjs.org/-/v1/search?text={escaped}&size={size}"
                ))
            }
            Self::Crates => {
                let per_page = limit.min(100);
                SearchSource::asking(format!(
                    "https://crates.io/api/v1/crates?q={escaped}&per_page={per_page}"
                ))
            }
            Self::PyPi => SearchSource {
                url: "https://pypi.org/simple/".to_owned(),
                accept: "application/vnd.pypi.simple.v1+json",
                whole_index: true,
                names_a_package: false,
            },
            Self::Go => SearchSource {
                url: format!(
                    "{GO_PROXY}/{}/@latest",
                    go_module_path(module_path_in(query))
                ),
                accept: "application/json",
                whole_index: false,
                names_a_package: true,
            },
            Self::Composer => {
                let per_page = limit.clamp(1, 100);
                SearchSource::asking(format!(
                    "https://packagist.org/search.json?q={escaped}&per_page={per_page}"
                ))
            }
        }
    }

    pub fn read_hits(self, body: &str, query: &str, limit: u32) -> Option<Vec<Hit>> {
        let hits: Vec<Hit> = match self {
            Self::Npm => {
                let answer: NpmSearch = serde_json::from_str(body).ok()?;
                answer
                    .objects
                    .into_iter()
                    .map(|object| Hit {
                        name: object.package.name,
                        version: object.package.version,
                        description: object.package.description,
                    })
                    .collect()
            }
            Self::Crates => {
                let answer: CratesSearch = serde_json::from_str(body).ok()?;
                answer
                    .crates
                    .into_iter()
                    .map(|found| Hit {
                        name: found.name,
                        version: found.default_version,
                        description: found.description,
                    })
                    .collect()
            }

            Self::PyPi => {
                let index: PyPiIndex = serde_json::from_str(body).ok()?;
                let query = query.to_lowercase();

                let mut matched: Vec<(Rank, &str)> = index
                    .projects
                    .iter()
                    .filter_map(|project| {
                        rank(&project.name, &query).map(|rank| (rank, project.name.as_str()))
                    })
                    .collect();

                matched.sort_by(|(left_rank, left), (right_rank, right)| {
                    left_rank
                        .cmp(right_rank)
                        .then_with(|| left.len().cmp(&right.len()))
                        .then_with(|| left.cmp(right))
                });

                matched
                    .into_iter()
                    .map(|(_, name)| Hit {
                        name: name.to_owned(),
                        version: None,
                        description: None,
                    })
                    .collect()
            }

            Self::Go => {
                let module = self.read_current(body)?;
                vec![Hit {
                    name: module_path_in(query).to_owned(),
                    version: Some(module.version),
                    description: None,
                }]
            }

            Self::Composer => {
                let answer: PackagistSearch = serde_json::from_str(body).ok()?;
                answer
                    .results
                    .into_iter()
                    .filter(|found| !found.only_provided)
                    .map(|found| Hit {
                        name: found.name,
                        version: None,
                        description: found.description,
                    })
                    .collect()
            }
        };

        Some(hits.into_iter().take(limit as usize).collect())
    }

    pub fn hosts(self) -> BTreeSet<String> {
        const PROBE: &str = "vendor/package";

        let mut urls = vec![self.versions(PROBE).url];
        if let Ok(archive) = self.archive(PROBE, "1.0.0") {
            urls.push(archive.url().to_owned());
        }
        urls.push(self.search(PROBE, 1).url);

        let mut hosts: BTreeSet<String> = urls
            .iter()
            .filter_map(|url| host_of(url))
            .map(str::to_owned)
            .collect();
        hosts.extend(self.listed_hosts().iter().map(|host| (*host).to_owned()));
        hosts
    }

    fn listed_hosts(self) -> &'static [&'static str] {
        match self {
            Self::Npm | Self::Crates | Self::Go => &[],
            Self::PyPi => &["files.pythonhosted.org"],
            Self::Composer => &["codeload.github.com", "bitbucket.org", "gitlab.com"],
        }
    }

    fn undated_order(self, left: &str, right: &str) -> Ordering {
        match self {
            Self::Npm | Self::Crates | Self::PyPi | Self::Composer => Ordering::Equal,
            Self::Go => GoVersion::parse(left).cmp(&GoVersion::parse(right)),
        }
    }

    pub fn is_prerelease(self, version: &str) -> bool {
        let version = version.split('+').next().unwrap_or_default();

        match self {
            Self::Npm | Self::Crates | Self::Go => version.contains('-'),
            Self::PyPi => pep440_prerelease(version),
            Self::Composer => composer_preview(version),
        }
    }

    pub fn choose_archive(self, version: &str, listing: &str) -> Option<String> {
        engine::choose_archive(self.id(), version, listing).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveSource {
    Archive { url: String },

    Listing { url: String },
}

impl ArchiveSource {
    pub fn url(&self) -> &str {
        match self {
            Self::Archive { url } | Self::Listing { url } => url,
        }
    }
}

impl Serialize for Registry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for Registry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = Cow::<str>::deserialize(deserializer)?;
        Self::from_id(&id).ok_or_else(|| {
            let known: Vec<&str> = Self::ALL.iter().map(|registry| registry.id()).collect();
            de::Error::custom(format!(
                "unknown registry `{id}`, expected one of: {}",
                known.join(", ")
            ))
        })
    }
}

impl JsonSchema for Registry {
    fn schema_name() -> Cow<'static, str> {
        "Registry".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Registry").into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "enum": Registry::ALL.map(|registry| registry.id()),
            "description": "The registry that publishes the package.",
        })
    }
}

pub const VERSION_RULE: &str = "A version is one published version, spelled the way the \
                                registry spells it: not a range, not a tag, and nothing \
                                normalised — `v4.0.0` and `4.0.0` are different versions.";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct PackageName(String);

impl PackageName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<PackageName> for String {
    fn from(name: PackageName) -> Self {
        name.0
    }
}

impl JsonSchema for PackageName {
    fn schema_name() -> Cow<'static, str> {
        "PackageName".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::PackageName").into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        let rules: Vec<String> = Registry::ALL
            .iter()
            .map(|registry| format!("{}: {}", registry.name(), registry.name_rule()))
            .collect();
        json_schema!({
            "type": "string",
            "description": format!(
                "The package name as the registry spells it, scope included: `zod`, \
                 `@types/node`, `serde`. Nothing is normalised, and what that means \
                 differs per registry. {} The same rules are served at \
                 `diffpack://registries`.",
                rules.join(" ")
            ),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct VersionName(String);

impl VersionName {
    pub fn new(version: impl Into<String>) -> Self {
        Self(version.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<VersionName> for String {
    fn from(version: VersionName) -> Self {
        version.0
    }
}

impl JsonSchema for VersionName {
    fn schema_name() -> Cow<'static, str> {
        "VersionName".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::VersionName").into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": format!(
                "{VERSION_RULE} `list_package_versions` lists the versions a package has."
            ),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSource {
    pub url: String,

    pub current: Option<String>,
}

impl VersionSource {
    fn one_document(url: String) -> Self {
        Self { url, current: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Versions {
    pub all: Vec<Version>,

    pub current: Option<String>,
}

impl Versions {
    pub fn only(version: Version) -> Self {
        Self {
            current: Some(version.version.clone()),
            all: vec![version],
        }
    }

    pub fn lists(&self, version: &str) -> bool {
        self.all
            .iter()
            .any(|listed| without_leading_v(&listed.version) == without_leading_v(version))
    }

    pub fn newest(&self, count: usize) -> Vec<String> {
        self.all
            .iter()
            .take(count)
            .map(|listed| listed.version.clone())
            .collect()
    }

    pub fn pointing_at(self, current: String) -> Self {
        Self {
            current: Some(current),
            ..self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub version: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSource {
    pub url: String,
    pub accept: &'static str,
    pub whole_index: bool,
    pub names_a_package: bool,
}

impl SearchSource {
    fn asking(url: String) -> Self {
        Self {
            url,
            accept: "application/json",
            whole_index: false,
            names_a_package: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub name: String,

    pub version: Option<String>,

    pub description: Option<String>,
}

#[derive(Deserialize)]
struct NpmSearch {
    objects: Vec<NpmObject>,
}

#[derive(Deserialize)]
struct NpmObject {
    package: NpmPackage,
}

#[derive(Deserialize)]
struct NpmPackage {
    name: String,
    version: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct CratesSearch {
    crates: Vec<CratesCrate>,
}

#[derive(Deserialize)]
struct CratesCrate {
    name: String,
    default_version: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct PackagistSearch {
    results: Vec<PackagistHit>,
}

#[derive(Deserialize)]
struct PackagistHit {
    name: String,
    description: Option<String>,
    #[serde(default, rename = "virtual")]
    only_provided: bool,
}

#[derive(Deserialize)]
struct PyPiIndex {
    projects: Vec<PyPiProject>,
}

#[derive(Deserialize)]
struct PyPiProject {
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Exact,
    Prefix,
    Contains,
}

fn rank(name: &str, query: &str) -> Option<Rank> {
    let name = name.to_lowercase();
    if name == query {
        Some(Rank::Exact)
    } else if name.starts_with(query) {
        Some(Rank::Prefix)
    } else if name.contains(query) {
        Some(Rank::Contains)
    } else {
        None
    }
}

#[derive(PartialEq, Eq)]
struct GoVersion<'a> {
    numbers: [u64; 3],
    prerelease: Option<&'a str>,
}

impl<'a> GoVersion<'a> {
    fn parse(version: &'a str) -> Self {
        let version = version.strip_prefix('v').unwrap_or(version);
        let without_build = version.split('+').next().unwrap_or_default();
        let (core, prerelease) = match without_build.split_once('-') {
            Some((core, prerelease)) => (core, Some(prerelease)),
            None => (without_build, None),
        };

        let mut numbers = [0; 3];
        for (slot, part) in numbers.iter_mut().zip(core.split('.')) {
            *slot = leading_number(part);
        }

        Self {
            numbers,
            prerelease,
        }
    }
}

impl Ord for GoVersion<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers
            .cmp(&other.numbers)
            .then_with(|| match (self.prerelease, other.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => left.cmp(right),
            })
    }
}

impl PartialOrd for GoVersion<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn leading_number(part: &str) -> u64 {
    let digits = part.len() - part.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    part[..digits].parse().unwrap_or_default()
}

fn pep440_prerelease(version: &str) -> bool {
    const MARKERS: [&str; 8] = ["a", "b", "c", "rc", "alpha", "beta", "pre", "preview"];

    let version = version.to_ascii_lowercase();

    let version = version.rsplit('!').next().unwrap_or_default();

    let tail = version.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let tail = tail.trim_start_matches(['.', '-', '_']);

    if tail.contains("dev") {
        return true;
    }

    MARKERS.iter().any(|marker| {
        tail.strip_prefix(marker).is_some_and(|after| {
            after.is_empty() || after.starts_with(|c: char| c.is_ascii_digit())
        })
    })
}

const PACKAGIST_MINIFIED: &str = "composer/2.0";

const PACKAGIST_UNSET: &str = "__unset";

#[derive(Deserialize)]
struct PackagistListing {
    #[serde(default)]
    minified: Option<String>,
    packages: std::collections::HashMap<String, Vec<PackagistEntry>>,
}

#[derive(Deserialize)]
struct PackagistEntry {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    version_normalized: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Default)]
struct PackagistRelease {
    version: Option<String>,
    version_normalized: Option<String>,
    time: Option<String>,
}

impl PackagistRelease {
    fn absorb(&mut self, entry: PackagistEntry) {
        carry_forward(&mut self.version, entry.version);
        carry_forward(&mut self.version_normalized, entry.version_normalized);
        carry_forward(&mut self.time, entry.time);
    }

    fn as_version(&self) -> Option<Version> {
        let version = self.version.clone()?;
        let stability_spelling = self.version_normalized.as_deref().unwrap_or(&version);
        Some(Version {
            prerelease: Registry::Composer.is_prerelease(stability_spelling),
            published_at: self.time.clone(),
            version,
        })
    }
}

fn packagist_versions(document: &str) -> Option<Vec<Version>> {
    let listing: PackagistListing = serde_json::from_str(document).ok()?;
    let carries_forward = listing.minified.as_deref() == Some(PACKAGIST_MINIFIED);
    let mut packages = listing.packages.into_values();
    let entries = packages.next().filter(|_| packages.next().is_none())?;

    let mut release = PackagistRelease::default();
    Some(
        entries
            .into_iter()
            .filter_map(|entry| {
                if !carries_forward {
                    release = PackagistRelease::default();
                }
                release.absorb(entry);
                release.as_version()
            })
            .collect(),
    )
}

fn carry_forward(carried: &mut Option<String>, listed: Option<String>) {
    match listed {
        Some(value) if value == PACKAGIST_UNSET => *carried = None,
        Some(value) => *carried = Some(value),
        None => {}
    }
}

fn without_leading_v(version: &str) -> &str {
    version.strip_prefix('v').unwrap_or(version)
}

fn composer_preview(version: &str) -> bool {
    const PREVIEWS: [&str; 6] = ["dev", "alpha", "beta", "rc", "a", "b"];

    let version = version.to_ascii_lowercase();
    let stability = version
        .trim_start_matches('v')
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
        .trim_start_matches(['-', '_', '.'])
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');

    stability.starts_with("dev") || stability.ends_with("dev") || PREVIEWS.contains(&stability)
}

const GO_PROXY: &str = "https://proxy.golang.org";

const PACKAGIST_REPOSITORY: &str = "https://repo.packagist.org";

fn composer_package_path(package: &str) -> String {
    match package.split_once('/') {
        Some((vendor, name)) => format!("{}/{}", escape(vendor), escape(name)),
        None => escape(package),
    }
}

fn module_path_in(query: &str) -> &str {
    const SCHEMES: [&str; 2] = ["https://", "http://"];

    let without_scheme = SCHEMES
        .iter()
        .find_map(|scheme| query.strip_prefix(scheme))
        .unwrap_or(query);
    without_scheme.trim_end_matches('/')
}

fn go_module_path(module: &str) -> String {
    const PATH_SEPARATOR_AND_CASE_MARK: &[u8] = b"/!";

    escape_keeping(
        &engine::escape_go_module_path(module),
        PATH_SEPARATOR_AND_CASE_MARK,
    )
}

fn escape(text: &str) -> String {
    escape_keeping(text, &[])
}

fn escape_keeping(text: &str, kept: &[u8]) -> String {
    let mut escaped = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                escaped.push(char::from(byte))
            }
            kept_byte if kept.contains(&kept_byte) => escaped.push(char::from(kept_byte)),
            other => escaped.push_str(&format!("%{other:02X}")),
        }
    }
    escaped
}

pub fn allowed_hosts() -> BTreeSet<String> {
    hosts_of(&Registry::ALL)
}

pub fn hosts_of(registries: &[Registry]) -> BTreeSet<String> {
    registries
        .iter()
        .flat_map(|registry| registry.hosts())
        .collect()
}

pub fn allows(url: &str) -> bool {
    match host_of(url) {
        Some(host) => allowed_hosts()
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host)),
        None => false,
    }
}

fn host_of(url: &str) -> Option<&str> {
    let authority = url.strip_prefix("https://")?;
    let authority = authority
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())?;

    if authority.contains(['@', ':']) {
        return None;
    }
    Some(authority)
}
