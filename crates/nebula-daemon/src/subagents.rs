//! SUBAGENT PRESENCE: which worktrees another session's subagents are
//! working in right now.
//!
//! A subagent is no session of nebula's: it has no row, no PTY, and its
//! hook traffic rides its parent's agent id. Its working directory is
//! deliberately never allowed to move the parent's row (`hooks` drops a
//! subagent payload's cwd), so a checkout a background subagent is
//! building in reads "nothing running" on the grid. This module puts that
//! work back on the worktree's own band: every subagent hook event names
//! the paths it touched — the payload's cwd, and the files and
//! directories in its `tool_input` — and each path that falls inside a
//! worktree other than the parent's own marks that worktree as worked in,
//! until the subagent stops, its parent goes, or it falls quiet for
//! [`IDLE_EXPIRY_MS`].

use nebula_core::{AgentId, SubagentPresence, WorktreeId};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// How long a subagent may go without a hook event in a worktree before
/// it is taken to be gone there — the backstop for a `SubagentStop` that
/// never came (a killed parent, a CLI that crashed mid-task). Generous on
/// purpose: Claude reports a tool call when it ends, so a single long
/// build is silence for its whole run.
pub const IDLE_EXPIRY_MS: i64 = 20 * 60 * 1000;

/// How stale a subscriber's "active Nm ago" may get before a new hook
/// event re-broadcasts it. Clients draw minutes, so finer is noise on the
/// socket — every tool call of every subagent would otherwise be a frame.
pub const ACTIVITY_RESOLUTION_MS: i64 = 30 * 1000;

/// The paths a subagent's hook event says it touched: its working
/// directory, the file or directory arguments of its tool call, and for a
/// shell command the absolute paths in it and the targets of `cd` and
/// `git -C`, relative ones resolved against the working directory.
/// Best effort — a path a command builds at run time is invisible here —
/// and harmless when wrong: a path in no worktree attributes nothing.
pub fn touched_paths(cwd: Option<&str>, tool_input: Option<&serde_json::Value>) -> Vec<PathBuf> {
    let base = cwd.map(PathBuf::from);
    let resolve = |p: &str| -> Option<PathBuf> {
        let p = expand_home(p.trim());
        if p.as_os_str().is_empty() {
            None
        } else if p.is_absolute() {
            Some(p)
        } else {
            base.as_ref().map(|b| b.join(p))
        }
    };
    let mut paths: Vec<PathBuf> = base.iter().cloned().collect();
    if let Some(input) = tool_input {
        for key in ["file_path", "path", "notebook_path"] {
            if let Some(p) = input.get(key).and_then(|v| v.as_str()).and_then(resolve) {
                paths.push(p);
            }
        }
        if let Some(command) = input.get("command").and_then(|v| v.as_str()) {
            paths.extend(command_paths(command).iter().filter_map(|p| resolve(p)));
        }
    }
    paths.dedup();
    paths
}

/// The path-like words of a shell command: every word that is an
/// absolute (or `~/`) path, and the word after `cd` or `git -C`. Words are
/// split on whitespace and shell punctuation, quotes dropped — enough for
/// the commands agents write, not a shell parser.
fn command_paths(command: &str) -> Vec<String> {
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '<' | '>'))
        .map(|w| w.trim_matches(|c| matches!(c, '"' | '\'' | '`')))
        .filter(|w| !w.is_empty())
        .collect();
    let mut found = Vec::new();
    for (i, word) in words.iter().enumerate() {
        // `--manifest-path=/x/Cargo.toml`, `-C/x`: the path after the flag.
        let value = word
            .split_once('=')
            .map_or(*word, |(_, v)| v)
            .trim_start_matches("-C");
        if value.starts_with('/') || value.starts_with("~/") {
            found.push(value.to_string());
            continue;
        }
        let after_cd = i > 0 && words[i - 1] == "cd";
        let after_git_c = i > 1 && words[i - 1] == "-C" && words[i - 2] == "git";
        if after_cd || after_git_c {
            found.push((*word).to_string());
        }
    }
    found
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|home| Path::new(&home).join(rest))
            .unwrap_or_else(|| PathBuf::from(p)),
        None => PathBuf::from(p),
    }
}

/// Each worktree's root under every spelling it has — as stored and as
/// resolved — for [`worktree_of`]. Built once per hook event.
pub fn worktree_roots(worktrees: &[(WorktreeId, PathBuf)]) -> Vec<(WorktreeId, PathBuf)> {
    worktrees
        .iter()
        .flat_map(|(id, root)| spellings(root).into_iter().map(move |r| (id.clone(), r)))
        .collect()
}

/// The deepest worktree containing `path`, among [`worktree_roots`] — a
/// checkout nested inside another (`repo/.claude/worktrees/x` under the
/// root) is its own worktree, never the root's. Both the given and the
/// resolved spelling of each side count, so `/tmp` vs `/private/tmp` and
/// a file that does not exist yet (a Write's target) still match.
pub fn worktree_of(path: &Path, roots: &[(WorktreeId, PathBuf)]) -> Option<WorktreeId> {
    let forms = spellings(path);
    roots
        .iter()
        .filter(|(_, root)| forms.iter().any(|p| p.starts_with(root)))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(id, _)| id.clone())
}

/// `path` as given and as resolved — the resolution of its deepest
/// existing ancestor, with the rest re-attached, so a path about to be
/// created resolves like its directory does.
fn spellings(path: &Path) -> Vec<PathBuf> {
    let mut forms = vec![path.to_path_buf()];
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(existing) {
            let resolved = rest
                .iter()
                .rev()
                .fold(real, |acc: PathBuf, part| acc.join(part));
            if resolved != path {
                forms.push(resolved);
            }
            break;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => break,
        }
    }
    forms
}

/// Every live subagent's worktrees, with when it was last seen in each.
#[derive(Debug, Default)]
pub struct SubagentTracker {
    /// (parent session, subagent id) → worktree → epoch ms last seen.
    seen: HashMap<(AgentId, String), HashMap<WorktreeId, i64>>,
    /// Per worktree, the newest activity subscribers were last told of —
    /// what [`ACTIVITY_RESOLUTION_MS`] is measured from.
    told: HashMap<WorktreeId, i64>,
    /// Parents whose process ended. A subagent event the hook drain loop
    /// reads after the exit (the two run apart) must not bring a dead
    /// session's subagent back; the parent's next process clears it.
    departed: HashSet<AgentId>,
}

impl SubagentTracker {
    /// A subagent's hook event touched `worktrees` at `now_ms`. Returns
    /// the worktrees whose presence subscribers should hear again: one
    /// this subagent is new to, or one whose last word is
    /// [`ACTIVITY_RESOLUTION_MS`] old.
    pub fn touch(
        &mut self,
        parent: &AgentId,
        subagent: &str,
        worktrees: &[WorktreeId],
        now_ms: i64,
    ) -> Vec<WorktreeId> {
        if worktrees.is_empty() || self.departed.contains(parent) {
            return Vec::new();
        }
        let seen = self
            .seen
            .entry((parent.clone(), subagent.to_string()))
            .or_default();
        let mut changed = Vec::new();
        for w in worktrees {
            let new_here = seen.insert(w.clone(), now_ms).is_none();
            let told = self.told.get(w).copied();
            if new_here || told.is_none_or(|t| now_ms - t >= ACTIVITY_RESOLUTION_MS) {
                self.told.insert(w.clone(), now_ms);
                if !changed.contains(w) {
                    changed.push(w.clone());
                }
            }
        }
        changed
    }

    /// The subagent stopped: its worktrees, to re-broadcast.
    pub fn stop(&mut self, parent: &AgentId, subagent: &str) -> Vec<WorktreeId> {
        let gone = self
            .seen
            .remove(&(parent.clone(), subagent.to_string()))
            .map(|w| w.into_keys().collect())
            .unwrap_or_default();
        self.forget_quiet(gone)
    }

    /// The parent session went (exited, archived, deleted): every
    /// subagent it had goes with it.
    pub fn drop_parent(&mut self, parent: &AgentId) -> Vec<WorktreeId> {
        self.departed.insert(parent.clone());
        let mut gone = Vec::new();
        self.seen.retain(|(p, _), worktrees| {
            if p == parent {
                gone.extend(worktrees.keys().cloned());
                false
            } else {
                true
            }
        });
        self.forget_quiet(gone)
    }

    /// The parent has a process again: its subagents count once more.
    pub fn revive(&mut self, parent: &AgentId) {
        self.departed.remove(parent);
    }

    /// Worktrees a subagent has been silent in for [`IDLE_EXPIRY_MS`]
    /// leave its list; the worktrees that changed, to re-broadcast.
    pub fn expire(&mut self, now_ms: i64) -> Vec<WorktreeId> {
        let mut gone = Vec::new();
        for worktrees in self.seen.values_mut() {
            worktrees.retain(|w, at| {
                let keep = now_ms - *at < IDLE_EXPIRY_MS;
                if !keep {
                    gone.push(w.clone());
                }
                keep
            });
        }
        self.seen.retain(|_, worktrees| !worktrees.is_empty());
        self.forget_quiet(gone)
    }

    /// A worktree went away: nothing is working in it any more.
    pub fn forget_worktree(&mut self, worktree: &WorktreeId) {
        for worktrees in self.seen.values_mut() {
            worktrees.remove(worktree);
        }
        self.seen.retain(|_, worktrees| !worktrees.is_empty());
        self.told.remove(worktree);
    }

    /// Dedup `gone`, dropping the told-stamp of each that is now empty.
    fn forget_quiet(&mut self, mut gone: Vec<WorktreeId>) -> Vec<WorktreeId> {
        gone.sort_by(|a, b| a.0.cmp(&b.0));
        gone.dedup();
        for w in &gone {
            if self.presence(w).is_empty() {
                self.told.remove(w);
            }
        }
        gone
    }

    /// Who is working in `worktree`: one entry per parent session, sorted
    /// by parent id so the list is stable.
    pub fn presence(&self, worktree: &WorktreeId) -> Vec<SubagentPresence> {
        let mut by_parent: HashMap<&AgentId, SubagentPresence> = HashMap::new();
        for ((parent, _), worktrees) in &self.seen {
            let Some(&at) = worktrees.get(worktree) else {
                continue;
            };
            let entry = by_parent.entry(parent).or_insert_with(|| SubagentPresence {
                parent: parent.clone(),
                subagents: 0,
                last_active_ms: at,
            });
            entry.subagents += 1;
            entry.last_active_ms = entry.last_active_ms.max(at);
        }
        let mut list: Vec<SubagentPresence> = by_parent.into_values().collect();
        list.sort_by(|a, b| a.parent.0.cmp(&b.parent.0));
        list
    }

    /// Every worktree some subagent is working in.
    pub fn worktrees(&self) -> Vec<WorktreeId> {
        let mut all: Vec<WorktreeId> = self.seen.values().flat_map(|w| w.keys().cloned()).collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all.dedup();
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wt(id: &str) -> WorktreeId {
        WorktreeId(id.into())
    }

    #[test]
    fn tool_input_paths_are_read_and_resolved_against_the_cwd() {
        let got = touched_paths(
            Some("/repo"),
            Some(&json!({"file_path": "/repo/.claude/worktrees/x/src/a.rs"})),
        );
        assert_eq!(
            got,
            [
                PathBuf::from("/repo"),
                PathBuf::from("/repo/.claude/worktrees/x/src/a.rs")
            ]
        );
        assert_eq!(
            touched_paths(Some("/repo"), Some(&json!({"path": "sub/dir"})))[1],
            PathBuf::from("/repo/sub/dir"),
            "Grep/Glob's relative path"
        );
        assert_eq!(
            touched_paths(None, Some(&json!({"notebook_path": "/n/a.ipynb"}))),
            [PathBuf::from("/n/a.ipynb")]
        );
        assert!(touched_paths(None, Some(&json!({"path": "rel"}))).is_empty());
    }

    #[test]
    fn shell_commands_yield_absolute_paths_cd_and_git_dash_c_targets() {
        let cmd = r#"cd ../wt && cargo test --manifest-path=/w/feat/Cargo.toml; git -C "/w/fix" status | tee out.txt; git -C sub log"#;
        assert_eq!(
            command_paths(cmd),
            ["../wt", "/w/feat/Cargo.toml", "/w/fix", "sub"]
        );
        let got = touched_paths(
            Some("/w/main"),
            Some(&json!({"command": "cd ../wt && ls ~/notes"})),
        );
        assert!(got.contains(&PathBuf::from("/w/main/../wt")), "{got:?}");
        assert!(command_paths("echo hi").is_empty());
    }

    #[test]
    fn a_path_belongs_to_the_deepest_worktree_containing_it() {
        let worktrees = vec![
            (wt("root"), PathBuf::from("/repo")),
            (wt("nested"), PathBuf::from("/repo/.claude/worktrees/x")),
            (wt("other"), PathBuf::from("/elsewhere/feat")),
        ];
        let roots = worktree_roots(&worktrees);
        let of = |p: &str| worktree_of(Path::new(p), &roots);
        assert_eq!(of("/repo/.claude/worktrees/x/src/a.rs"), Some(wt("nested")));
        assert_eq!(of("/repo/src/main.rs"), Some(wt("root")));
        assert_eq!(of("/elsewhere/feature/x"), None, "whole components only");
        assert_eq!(of("/nowhere"), None);
    }

    #[test]
    fn a_new_file_under_a_symlinked_root_still_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(tmp.path()).unwrap().join("feat");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let worktrees = vec![(wt("feat"), real.clone())];
        assert_eq!(
            worktree_of(
                &link.join("not-yet/created.rs"),
                &worktree_roots(&worktrees)
            ),
            Some(wt("feat"))
        );
    }

    #[test]
    fn presence_counts_subagents_per_parent_until_they_stop() {
        let mut t = SubagentTracker::default();
        let (p1, p2) = (AgentId("p1".into()), AgentId("p2".into()));
        assert_eq!(t.touch(&p1, "s1", &[wt("w")], 1_000), [wt("w")]);
        assert_eq!(
            t.touch(&p1, "s2", &[wt("w")], 2_000),
            [wt("w")],
            "new subagent"
        );
        assert_eq!(
            t.touch(&p2, "s9", &[wt("w"), wt("v")], 3_000),
            [wt("w"), wt("v")]
        );
        let w = t.presence(&wt("w"));
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].subagents, w[0].last_active_ms), (2, 2_000));
        assert_eq!((w[1].subagents, w[1].last_active_ms), (1, 3_000));

        assert_eq!(t.stop(&p1, "s1"), [wt("w")]);
        assert_eq!(t.presence(&wt("w"))[0].subagents, 1);
        assert_eq!(t.drop_parent(&p2), [wt("v"), wt("w")]);
        assert!(t.presence(&wt("v")).is_empty());
        assert_eq!(t.worktrees(), [wt("w")]);
        assert!(t.stop(&p1, "unknown").is_empty());

        // A late event from the gone parent's subagent brings nothing back
        // — until the parent has a process again.
        assert!(t.touch(&p2, "s9", &[wt("v")], 4_000).is_empty());
        assert!(t.presence(&wt("v")).is_empty());
        t.revive(&p2);
        assert_eq!(t.touch(&p2, "s9", &[wt("v")], 5_000), [wt("v")]);
    }

    #[test]
    fn a_busy_subagent_is_re_announced_only_at_the_resolution() {
        let mut t = SubagentTracker::default();
        let p = AgentId("p".into());
        assert_eq!(t.touch(&p, "s", &[wt("w")], 0), [wt("w")]);
        assert!(t
            .touch(&p, "s", &[wt("w")], ACTIVITY_RESOLUTION_MS - 1)
            .is_empty());
        assert_eq!(
            t.touch(&p, "s", &[wt("w")], ACTIVITY_RESOLUTION_MS),
            [wt("w")]
        );
        assert_eq!(
            t.presence(&wt("w"))[0].last_active_ms,
            ACTIVITY_RESOLUTION_MS
        );
        assert!(t.touch(&p, "s", &[], 99).is_empty());
    }

    #[test]
    fn a_silent_subagent_expires_and_a_deleted_worktree_is_forgotten() {
        let mut t = SubagentTracker::default();
        let p = AgentId("p".into());
        t.touch(&p, "s", &[wt("old")], 0);
        t.touch(&p, "s", &[wt("new")], IDLE_EXPIRY_MS / 2);
        assert_eq!(t.expire(IDLE_EXPIRY_MS), [wt("old")]);
        assert_eq!(t.worktrees(), [wt("new")]);
        t.forget_worktree(&wt("new"));
        assert!(t.worktrees().is_empty());
        // A stamp left behind would hush the next subagent's first word.
        assert_eq!(t.touch(&p, "s2", &[wt("new")], 1), [wt("new")]);
    }
}
