//! ISSUE SOURCES: the programs a repository names in git config
//! (`nebula.issueSource`, one value per source) whose issues list beside
//! GitHub's. Read the same way by the TUI, which runs them, and the DAEMON,
//! which checks a launch names one of them — so the two can never disagree
//! about which programs a repository configures.

/// The git config key naming a repository's ISSUE SOURCE programs.
pub const KEY: &str = "nebula.issueSource";

/// The `git` arguments that read every value of [`KEY`], resolved the way
/// git resolves any multi-valued key (system, global, then the repo's
/// own). `--type=path` expands a leading `~/` as git does for any path.
pub const GIT_ARGS: [&str; 4] = ["config", "--type=path", "--get-all", KEY];

/// What git config names, parsed out of [`GIT_ARGS`]' output.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Configured {
    /// The programs nebula may run: absolute paths, each once, in git's
    /// order.
    pub programs: Vec<String>,
    /// Values it won't run: a relative path would resolve against whichever
    /// checkout happened to run it — the main checkout's current branch in
    /// the background, an agent's worktree in its prompt — so it is never
    /// taken as a program.
    pub refused: Vec<String>,
}

impl Configured {
    pub fn parse(git_stdout: &str) -> Self {
        let mut configured = Configured::default();
        for value in git_stdout.lines().map(str::trim).filter(|v| !v.is_empty()) {
            let list = if std::path::Path::new(value).is_absolute() {
                &mut configured.programs
            } else {
                &mut configured.refused
            };
            if !list.iter().any(|v| v == value) {
                list.push(value.to_string());
            }
        }
        configured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_run_once_each_and_relative_ones_never() {
        let configured =
            Configured::parse("/opt/bin/a\n\n  /opt/bin/b \nscripts/issues\n/opt/bin/a\n~/bin/c\n");
        assert_eq!(configured.programs, ["/opt/bin/a", "/opt/bin/b"]);
        assert_eq!(configured.refused, ["scripts/issues", "~/bin/c"]);
        assert_eq!(Configured::parse(""), Configured::default());
    }
}
