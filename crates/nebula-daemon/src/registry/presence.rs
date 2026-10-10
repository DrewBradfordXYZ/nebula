use super::*;
use crate::hooks::SubagentReport;
use crate::subagents;
use nebula_core::SubagentPresence;

impl Daemon {
    /// A subagent's hook event: SUBAGENT PRESENCE learns which worktrees
    /// it is working in — other than its parent's own, where the parent's
    /// card already says work is under way — or that it stopped, and
    /// subscribers hear of each worktree whose list changed. Fail-soft:
    /// any error leaves presence as it was.
    pub fn note_subagent(&self, parent: &AgentId, event: &HookEvent, report: SubagentReport) {
        let changed = if matches!(event, HookEvent::SubagentStop { .. }) {
            self.subagents.lock().stop(parent, &report.id)
        } else {
            match self.subagent_worktrees(parent, &report.paths) {
                Ok(worktrees) => self.subagents.lock().touch(
                    parent,
                    &report.id,
                    &worktrees,
                    nebula_core::clock::now_ms(),
                ),
                Err(e) => {
                    tracing::warn!(agent = %parent, error = %e, "subagent presence lookup failed");
                    return;
                }
            }
        };
        self.broadcast_presence(changed);
    }

    /// The worktrees `paths` fall in, the parent's own left out. An id
    /// with no row (a prewarm spare) attributes nothing.
    fn subagent_worktrees(&self, parent: &AgentId, paths: &[PathBuf]) -> Result<Vec<WorktreeId>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let Some(agent) = self.store.get_agent(parent)? else {
            return Ok(Vec::new());
        };
        let (_, worktrees, _, _) = self.store.load_tree()?;
        let pairs: Vec<(WorktreeId, PathBuf)> =
            worktrees.into_iter().map(|w| (w.id, w.path)).collect();
        let roots = subagents::worktree_roots(&pairs);
        let mut found: Vec<WorktreeId> = Vec::new();
        for path in paths {
            if let Some(w) = subagents::worktree_of(path, &roots) {
                if w != agent.worktree_id && !found.contains(&w) {
                    found.push(w);
                }
            }
        }
        Ok(found)
    }

    /// Who is working in `worktree` from another session, right now.
    pub fn subagent_presence(&self, worktree: &WorktreeId) -> Vec<SubagentPresence> {
        self.subagents.lock().presence(worktree)
    }

    /// One `WorktreeActivity` per worktree some subagent is working in —
    /// what a client that just took a `Snapshot` needs to catch up.
    pub fn presence_events(&self) -> Vec<ServerEvent> {
        let tracker = self.subagents.lock();
        tracker
            .worktrees()
            .into_iter()
            .map(|worktree| ServerEvent::WorktreeActivity {
                agents: tracker.presence(&worktree),
                worktree,
            })
            .collect()
    }

    /// Subagents silent past `subagents::IDLE_EXPIRY_MS` — a stop that
    /// never came — leave the bands they were on.
    pub fn expire_subagents(&self) {
        let changed = self.subagents.lock().expire(nebula_core::clock::now_ms());
        self.broadcast_presence(changed);
    }

    /// The parent has a process again (spawned, respawned, resumed).
    pub(super) fn subagents_may_return(&self, parent: &AgentId) {
        self.subagents.lock().revive(parent);
    }

    /// The parent's process ended: its subagents ran inside it.
    pub(super) fn subagents_gone_with(&self, parent: &AgentId) {
        let changed = self.subagents.lock().drop_parent(parent);
        self.broadcast_presence(changed);
    }

    /// A worktree left nebula: nothing can be working in it any more.
    /// Clients drop its row, and with it what they showed on it.
    pub(super) fn forget_worktree_presence(&self, worktree: &WorktreeId) {
        self.subagents.lock().forget_worktree(worktree);
    }

    fn broadcast_presence(&self, worktrees: Vec<WorktreeId>) {
        for worktree in worktrees {
            let agents = self.subagent_presence(&worktree);
            self.broadcast(ServerEvent::WorktreeActivity { worktree, agents });
        }
    }
}
