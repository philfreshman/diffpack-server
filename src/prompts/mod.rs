use rmcp::model::{
    GetPromptResult, JsonObject, Prompt as Definition, PromptArgument, PromptMessage,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::Failure;

macro_rules! prompts {
    ($($module:ident::$prompt:ident),+ $(,)?) => {
        $(pub mod $module;)+

        pub fn definitions() -> Vec<Definition> {
            let mut prompts = vec![$(definition::<$module::$prompt>()),+];
            prompts.sort_by(|a, b| a.name.cmp(&b.name));
            prompts
        }

        pub fn get(name: &str, arguments: Option<JsonObject>) -> Result<GetPromptResult, Failure> {
            match name {
                $(<$module::$prompt as Prompt>::NAME => render::<$module::$prompt>(arguments),)+
                unknown => Err(Failure::NoSuchPrompt {
                    name: unknown.to_owned(),
                }),
            }
        }
    };
}

prompts! {
    review_upgrade::ReviewUpgrade,
}

pub trait Prompt {
    const NAME: &'static str;

    const TITLE: &'static str;

    const DESCRIPTION: &'static str;

    type Args: DeserializeOwned;

    fn arguments() -> Vec<PromptArgument>;

    fn messages(args: Self::Args) -> Vec<PromptMessage>;
}

pub fn required(name: &str, title: &str, description: &str) -> PromptArgument {
    PromptArgument::new(name)
        .with_title(title)
        .with_description(description)
        .with_required(true)
}

fn definition<P: Prompt>() -> Definition {
    Definition::new(P::NAME, Some(P::DESCRIPTION), Some(P::arguments())).with_title(P::TITLE)
}

fn render<P: Prompt>(arguments: Option<JsonObject>) -> Result<GetPromptResult, Failure> {
    let args = serde_json::from_value::<P::Args>(Value::Object(filled_in(arguments))).map_err(
        |invalid| Failure::InvalidParams {
            message: invalid.to_string(),
        },
    )?;

    Ok(GetPromptResult::new(P::messages(args)))
}

fn filled_in(arguments: Option<JsonObject>) -> JsonObject {
    arguments
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, value)| !is_blank(value))
        .collect()
}

fn is_blank(value: &Value) -> bool {
    value.as_str().is_some_and(|text| text.trim().is_empty())
}
