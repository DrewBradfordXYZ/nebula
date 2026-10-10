//! SUBAGENT PRESENCE as the grid shows it: a worktree another session's
//! subagents are working in says so on its band — `1 agent from 'Fix
//! Login' · active 2m ago` where an empty band would read "nothing
//! running", and `· 1 agent` beside the session count on its rule — and
//! the DELETE CHECK's confirm names them too. The DAEMON tracks it
//! (`ServerEvent::WorktreeActivity`); this module only keeps and words it.

use crate::app::App;
use nebula_core::{SubagentPresence, WorktreeId};

/// Parent sessions a label names before it says "+N more".
const NAMED_PARENTS: usize = 2;

/// The DAEMON's word on who is working in `worktree`; an empty list
/// clears it.
pub(crate) fn land(app: &mut App, worktree: WorktreeId, agents: Vec<SubagentPresence>) {
    if agents.is_empty() {
        app.tree.subagents.remove(&worktree);
    } else {
        app.tree.subagents.insert(worktree, agents);
    }
    app.chrome.dirty = true;
}

/// How many other sessions' subagents are working in `worktree`.
pub(crate) fn count(app: &App, worktree: &WorktreeId) -> u32 {
    app.tree
        .subagents
        .get(worktree)
        .map_or(0, |list| list.iter().map(|p| p.subagents).sum())
}

/// `N agent(s)` — the rule's word for [`count`].
pub(crate) fn agents_word(n: u32) -> String {
    format!("{n} agent{}", if n == 1 { "" } else { "s" })
}

/// `1 agent from 'Fix Login' · active 2m ago`: who is working in
/// `worktree` and how recently, or None when nobody is.
pub(crate) fn label(app: &App, worktree: &WorktreeId, now_ms: i64) -> Option<String> {
    let list = app.tree.subagents.get(worktree)?;
    let n = count(app, worktree);
    let mut parents: Vec<String> = list
        .iter()
        .take(NAMED_PARENTS)
        .map(|p| format!("'{}'", parent_name(app, p)))
        .collect();
    if list.len() > NAMED_PARENTS {
        parents.push(format!("+{} more", list.len() - NAMED_PARENTS));
    }
    let newest = list.iter().map(|p| p.last_active_ms).max()?;
    let mut text = format!("{} from {}", agents_word(n), parents.join(", "));
    let ago = crate::hosts::ago_label(now_ms - newest);
    if !ago.is_empty() {
        text.push_str(&format!(" · active {ago}"));
    }
    Some(text)
}

/// The parent session's name as its row shows it.
fn parent_name(app: &App, p: &SubagentPresence) -> String {
    app.tree
        .agents
        .iter()
        .find(|a| a.id == p.parent)
        .map_or_else(|| "another session".into(), |a| a.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::AgentId;

    fn presence(parent: &str, subagents: u32, at: i64) -> SubagentPresence {
        SubagentPresence {
            parent: AgentId(parent.into()),
            subagents,
            last_active_ms: at,
        }
    }

    #[test]
    fn a_label_names_parents_counts_subagents_and_says_how_recent() {
        let mut app = App::new();
        let w = WorktreeId("w".into());
        assert_eq!(label(&app, &w, 0), None);
        let now = 10_000_000;
        land(&mut app, w.clone(), vec![presence("p1", 1, now - 120_000)]);
        assert_eq!(
            label(&app, &w, now).as_deref(),
            Some("1 agent from 'another session' · active 2m ago")
        );
        land(
            &mut app,
            w.clone(),
            vec![
                presence("p1", 2, now - 120_000),
                presence("p2", 1, now - 5_000),
                presence("p3", 1, now - 60_000),
            ],
        );
        assert_eq!(count(&app, &w), 4);
        assert_eq!(
            label(&app, &w, now).as_deref(),
            Some("4 agents from 'another session', 'another session', +1 more · active just now")
        );
        land(&mut app, w.clone(), vec![]);
        assert_eq!(count(&app, &w), 0);
        assert!(app.tree.subagents.is_empty());
    }
}
