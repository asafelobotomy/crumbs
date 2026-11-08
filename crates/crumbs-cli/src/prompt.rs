//! Terminal prompts. Callers check [`is_interactive`] first: prompts only
//! happen when both stdin and stdout are terminals.

use std::io::IsTerminal;

use dialoguer::{Confirm, MultiSelect};

pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Asks `<question> [y/N]`. Escape or Ctrl-C counts as no.
pub fn confirm(question: &str) -> anyhow::Result<bool> {
    let answer = Confirm::new()
        .with_prompt(question)
        .default(false)
        .interact_opt()?;
    Ok(answer == Some(true))
}

/// Lets the user tick items; `None` when the prompt was cancelled.
pub fn multi_select(
    prompt: &str,
    items: &[String],
    defaults: &[bool],
) -> anyhow::Result<Option<Vec<usize>>> {
    Ok(MultiSelect::new()
        .with_prompt(prompt)
        .items(items)
        .defaults(defaults)
        .interact_opt()?)
}
