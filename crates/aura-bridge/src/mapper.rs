use aura_llm::intent::Intent;

use crate::actions::Action;

/// Maps a parsed Intent to an executable Action.
/// Returns None for intents that don't map to system actions (e.g., SummarizeScreen, Unknown).
pub fn intent_to_action(intent: &Intent) -> Option<Action> {
    match intent {
        Intent::OpenApp { name } => Some(Action::OpenApp {
            name: name.clone(),
        }),
        Intent::SearchFiles { query } => Some(Action::SearchFiles {
            query: query.clone(),
        }),
        Intent::TileWindows { layout } => Some(Action::TileWindows {
            layout: layout.clone(),
        }),
        Intent::LaunchUrl { url } => Some(Action::LaunchUrl {
            url: url.clone(),
        }),
        Intent::SummarizeScreen => None,
        Intent::Unknown { .. } => None,
    }
}
