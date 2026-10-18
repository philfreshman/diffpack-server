use rmcp::model::{PromptArgument, PromptMessage, Role};
use serde::Deserialize;
use serde_json::json;

use crate::prompts::{required, Prompt};
use crate::registry::{PackageName, Registry, VersionName, VERSION_RULE};
use crate::tools::diff_package_versions::DiffPackageVersions;
use crate::tools::get_diff_tree::GetDiffTree;
use crate::tools::get_file_content::GetFileContent;
use crate::tools::get_file_diff::GetFileDiff;
use crate::tools::list_package_versions::ListPackageVersions;
use crate::tools::Tool;

pub struct ReviewUpgrade;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub registry: Registry,

    pub package: PackageName,

    pub from_version: VersionName,

    pub to_version: VersionName,
}

impl Prompt for ReviewUpgrade {
    const NAME: &'static str = "review_upgrade";

    const TITLE: &'static str = "Review a package upgrade";

    const DESCRIPTION: &'static str = "\
        Review what changed between two published versions of a package and \
        what could break for code that depends on it: removed exports, \
        changed signatures, new required options and dependency changes. \
        Walks an agent through comparing the versions, listing what changed \
        and reading the diffs that matter, in that order.";

    type Args = Args;

    fn arguments() -> Vec<PromptArgument> {
        vec![
            required("registry", "Registry", &registry_rule()),
            required(
                "package",
                "Package",
                "The package name as the registry spells it, scope included: \
                 `zod`, `@types/node`, `serde`.",
            ),
            required(
                "from_version",
                "From version",
                &format!("The version you are upgrading from. {VERSION_RULE}"),
            ),
            required(
                "to_version",
                "To version",
                &format!("The version you are upgrading to. {VERSION_RULE}"),
            ),
        ]
    }

    fn messages(args: Args) -> Vec<PromptMessage> {
        let review = [
            ask(&args),
            compare(&args),
            list_changes(),
            read_diffs(args.registry),
            report(args.registry),
        ]
        .join("\n\n");

        vec![PromptMessage::new_text(Role::User, review)]
    }
}

fn registry_rule() -> String {
    let ids: Vec<String> = Registry::ALL
        .iter()
        .map(|registry| format!("`{}`", registry.id()))
        .collect();

    format!(
        "The registry that publishes the package, by the identifier this \
         server names it with: one of {}.",
        ids.join(", ")
    )
}

fn ask(args: &Args) -> String {
    format!(
        "Review the upgrade of `{package}` on {registry} from `{from}` to `{to}`, \
         and tell me what could break for code that depends on it. Work \
         through the steps below in order, with this server's tools.",
        package = args.package.as_str(),
        registry = args.registry.name(),
        from = args.from_version.as_str(),
        to = args.to_version.as_str(),
    )
}

fn compare(args: &Args) -> String {
    let arguments = json!({
        "registry": args.registry,
        "package": args.package.as_str(),
        "from_version": args.from_version.as_str(),
        "to_version": args.to_version.as_str(),
    });

    format!(
        "1. Compare the two versions. Call `{compare}` with these arguments, \
         exactly as they are written here:\n\
         \n\
         ```json\n\
         {arguments}\n\
         ```\n\
         \n\
         `from_version` is the version being upgraded from and `to_version` \
         the one being upgraded to. Do not swap them: swapped, every addition \
         reads as a removal. A version is exact, so if either one is refused \
         as unknown, call `{versions}` to see how {registry} spells the \
         versions it has rather than guessing. The answer's `totals` say how \
         much changed and `most_changed` names the files that changed most. \
         Keep its `handle`: every step below takes it, passed back exactly as \
         you got it and never built or edited by hand.",
        compare = DiffPackageVersions::NAME,
        versions = ListPackageVersions::NAME,
        registry = args.registry.name(),
    )
}

fn list_changes() -> String {
    format!(
        "2. List everything that changed. Call `{tree}` with the handle and \
         `status` set to `added`, `removed`, `modified` and `renamed`, and \
         pass each answer's `nextCursor` back as `cursor` until there is \
         none. A large `unchanged` count is normal: most of a package stays \
         the same between two versions, so do not ask for those files and do \
         not report them.",
        tree = GetDiffTree::NAME,
    )
}

fn read_diffs(registry: Registry) -> String {
    format!(
        "3. Read the diffs that matter. Call `{file_diff}` with the handle and \
         a file's `path`. Start with what a consumer of the package touches: \
         {manifest}, where its dependencies and the {runtime} versions it \
         supports are declared; the entry points, public modules and type \
         declarations it exports; and its changelog, if it ships one. Tests, \
         examples, documentation and generated or minified files rarely \
         change what a consumer sees, so leave them unless something else \
         points at them. A `renamed` file is one file that moved, not a \
         removal and an addition: pass its `old_path` as well, and treat the \
         move itself as a possible break for anyone who imports it by path. \
         When a diff does not show enough of the code around a change, \
         `{content}` reads the whole file from either version.",
        file_diff = GetFileDiff::NAME,
        content = GetFileContent::NAME,
        manifest = registry.manifest(),
        runtime = registry.runtime(),
    )
}

fn report(registry: Registry) -> String {
    format!(
        "4. Report what could break for a consumer, under these headings where \
         they apply:\n\
         \n\
         - removed or renamed exports, modules and files that code imports\n\
         - changed function, method or type signatures\n\
         - new required options or arguments, and changed defaults\n\
         - dependency changes: dependencies added, removed or moved to a new \
         major version, and a raised minimum {runtime} version\n\
         - behaviour that changed while its signature stayed the same\n\
         \n\
         Name the file behind each point. Say which changes need a consumer \
         to act and which do not, and end with a one-line verdict on how \
         risky the upgrade is.",
        runtime = registry.runtime(),
    )
}
