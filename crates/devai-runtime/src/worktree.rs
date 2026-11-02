//! Platform-owned worktree isolation for writing roles (REQ-WT-01/06/08).
//!
//! Pure git plumbing — no workflow logic, no judgment (the sovereignty
//! precedent of [`crate::revision`]). The agent never branches or commits:
//! agents keep read-only git (adapters map the `git` capability to
//! status/diff/log only), and these functions are the ONLY way a branch or
//! a commit comes to exist during a run.
//!
//! REQ-WT-08 is the hard rule this module exists to satisfy: platform code
//! has no path that writes into the main checkout's WORKING TREE. The only
//! writes these functions cause anywhere under the main checkout root are
//! the two enumerated channels:
//!
//! * **(i)** the per-worktree administrative files that `git worktree
//!   add`/`prune` maintain under `.git/worktrees/<id>/` (including that
//!   worktree's own HEAD and index copies);
//! * **(ii)** the task's own branch ref `refs/heads/rework/<task-id>` —
//!   its creation here and its update by [`commit_platform_work`] — plus
//!   the git objects those commits require in the shared object store.
//!
//! Anything else under the checkout — working-tree files, HEAD, the
//! baseline branch's ref, `.git/index`, `.git/config`, stash, hooks, other
//! refs — is never touched: a new write channel would require revising the
//! specification, not this code.

use std::path::{Path, PathBuf};
use std::process::Command;

use devai_core::types::Id;

/// Branch shape mandated by the business: `rework/<task-slug>`. The slug is
/// the task id — a UUID, collision-free for tasks with identical titles and
/// already the identifier every artifact reference carries (this is the
/// platform's answer to OQ-2; the shape `rework/` is spelled NOWHERE else:
/// every consumer calls this function).
pub fn branch_name(task_id: Id) -> String {
    format!("rework/{task_id}")
}

/// The isolation directory for a task: on the DEVAI_HOME side of the wall,
/// OUTSIDE the main checkout's working tree (assumption A4 / interim answer
/// to OQ-8), so worktree contents can never appear as untracked files in
/// the user's checkout, and per-worktree build output (`target/`) cannot
/// collide across concurrent runs (REQ-WT-09 AC3).
pub fn path_for(home: &Path, task_id: Id) -> PathBuf {
    home.join("worktrees").join(task_id.to_string())
}

/// A prepared isolation worktree: the directory the writing execution runs
/// in, and the branch the platform owns there.
#[derive(Clone, Debug)]
pub struct Worktree {
    pub dir: PathBuf,
    pub branch: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("`{0}` is not a git repository — a writing stage cannot be isolated without history to branch from; the run fails instead of falling back (REQ-WT-08 hard rule)")]
    NotGit(PathBuf),
    #[error("the task is bound to no workspace revision commit — the writing stage fails explicitly instead of silently using the main checkout's current state (REQ-WT-01 AC2)")]
    NoBoundCommit,
    #[error("git {cmd} failed: {reason}")]
    Git { cmd: String, reason: String },
    #[error("{dir} exists but is checked out on `{found}`, not the task's `{expected}` branch — refusing to run a writing stage against a foreign tree")]
    BranchMismatch { dir: String, expected: String, found: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type WorktreeResult<T> = Result<T, WorktreeError>;

/// Committer identity the platform stamps its own commits with. Interim
/// answer to OQ-5 (the business does not fix an identity): a clearly
/// platform-owned address so a human can always tell a platform commit from
/// their own, and never the ambient user config (revision.rs pins its test
/// identity for the same reason). Passed via `git -c`, never `git config`.
const PLATFORM_COMMITTER_NAME: &str = "devai-platform";
const PLATFORM_COMMITTER_EMAIL: &str = "devai@platform.local";

/// Subject prefix marking platform commits. Second half of the interim OQ-5
/// answer; the body carries the task id and the execution id verbatim.
pub const COMMIT_SUBJECT_PREFIX: &str = "devai: implementation";

fn git(dir: &Path, args: &[&str]) -> WorktreeResult<String> {
    let out = Command::new("git").args(args).current_dir(dir).output()?;
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(WorktreeError::Git { cmd: format!("git {}", args.join(" ")), reason: stderr });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git").args(args).current_dir(dir).output().is_ok_and(|o| o.status.success())
}

/// True when `repo` is usable as the repository a worktree branches from.
/// `git worktree add` runs inside it and registers under its `.git/`
/// (channel (i)) — the working tree itself is never written.
pub fn is_git_repo(repo: &Path) -> bool {
    git_ok(repo, &["rev-parse", "--git-dir"])
}

/// True when the task's branch already exists (a rework run, or a QA probe
/// run after a writing stage). Checked as a ref, not as a directory: the
/// branch is the durable fact, the directory is reconstructible (REQ-WT-07
/// AC3: the branch persists until a human merges).
pub fn has_branch(repo: &Path, task_id: Id) -> bool {
    git_ok(repo, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{}", branch_name(task_id))])
}

/// The branch currently checked out in `dir`, if it is a git checkout at
/// all. Detached HEAD reads as an empty name (mismatch for our purposes).
pub fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["branch", "--show-current"])
        .ok()
        .filter(|s| !s.is_empty())
}

/// Create (or reuse) the task's isolated worktree BEFORE any writing-role
/// agent, gate or probe runs (REQ-WT-01). The choice is genuinely binary —
/// `has_branch` — never a guess about "whose directory is this":
///
/// * branch absent: `git worktree add <dir> -b rework/<task-id> <H>` with H
///   = the git_head component of the task's bound workspace_revision. A
///   bound revision without a commit (non-git workspace) is an explicit
///   failure — never a fallback to the main checkout's current state
///   (AC2). A non-empty dirty_digest is NOT transported: `git worktree add`
///   checks out commits, so the worktree branches from H alone (AC1 names
///   exactly the git_head component; the dirty-state question stays open
///   as OQ-3).
/// * branch present: reuse it — the worktree is re-attached to the same
///   directory (after `git worktree prune` clears a stale registration if
///   a human deleted the directory; prune maintains only channel (i)
///   admin files). The committed history of the task's own branch is
///   preserved, which is what a REWORK continues on.
/// * directory present: verified to be exactly the task's branch — a
///   foreign tree there is [`WorktreeError::BranchMismatch`], never silently
///   reused.
pub fn prepare(repo: &Path, home: &Path, task_id: Id, bound_head: Option<&str>) -> WorktreeResult<Worktree> {
    if !is_git_repo(repo) {
        return Err(WorktreeError::NotGit(repo.to_path_buf()));
    }
    let branch = branch_name(task_id);
    let dir = path_for(home, task_id);

    if dir.exists() {
        let found = current_branch(&dir).unwrap_or_default();
        if found != branch {
            return Err(WorktreeError::BranchMismatch {
                dir: dir.display().to_string(),
                expected: branch,
                found: if found.is_empty() { "(not a git checkout)".into() } else { found },
            });
        }
        return Ok(Worktree { dir, branch });
    }

    if has_branch(repo, task_id) {
        // Reuse: the directory is gone (deleted, or never created under this
        // home) but the branch — the durable fact — exists. `git worktree
        // prune` clears stale admin registrations (channel (i)), then the
        // branch is re-attached: nothing is CREATED, and a branch checked
        // out in a still-live other worktree honestly fails the add (git
        // refuses it) rather than corrupting that run's isolation.
        git(repo, &["worktree", "prune"])?;
        git(repo, &["worktree", "add", &dir.display().to_string(), &branch])?;
        return Ok(Worktree { dir, branch });
    }

    let head = bound_head.ok_or(WorktreeError::NoBoundCommit)?;
    // The bound revision's commit must actually exist in the object store;
    // `worktree add` verifies, but a clear typed error beats a git one.
    if !git_ok(repo, &["rev-parse", "--verify", &format!("{head}^0")]) {
        return Err(WorktreeError::Git { cmd: format!("git rev-parse --verify {head}^0"), reason: "the bound commit does not exist in the repository".into() });
    }
    git(repo, &["worktree", "add", &dir.display().to_string(), "-b", &branch, head])?;
    Ok(Worktree { dir, branch })
}

/// Stage everything the agent changed inside the worktree and commit it on
/// the task's own branch (REQ-WT-06). This is the only platform path that
/// moves `refs/heads/rework/<task-id>` (channel (ii)); its `git -C worktree`
/// operations can by construction touch nothing else: the worktree's HEAD
/// file lives under `.git/worktrees/<id>/` (channel (i)), and the baseline
/// branch, the main HEAD and the main index are not addressed at all.
///
/// Returns the full SHA of the resulting commit. When the agent changed
/// nothing, no commit is created (an empty commit would be a fake artifact);
/// the current branch tip — the commit that already captures the work — is
/// returned, which is what REQ-WT-04 AC2/AC3 asks the report to name: a
/// resolvable commit on the task's branch.
///
/// The commit message is the interim OQ-5 answer: subject
/// `<prefix>: <task title>` with a body citing the task and execution ids.
pub fn commit_platform_work(dir: &Path, subject: &str, task_id: Id, execution_id: Id) -> WorktreeResult<String> {
    git(dir, &["add", "-A"])?;
    // `diff --cached --quiet`: exit 1 means staged content exists; 0 means
    // the worktree matches the branch tip and there is nothing to capture.
    let dirty = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(dir)
        .output()?;
    if !dirty.status.success() {
        git(
            dir,
            &[
                "-c",
                &format!("user.name={PLATFORM_COMMITTER_NAME}"),
                "-c",
                &format!("user.email={PLATFORM_COMMITTER_EMAIL}"),
                "commit",
                "-q",
                "--no-gpg-sign",
                "-m",
                &format!("{subject}\n\ntask: {task_id}\nexecution: {execution_id}"),
            ],
        )?;
    }
    git(dir, &["rev-parse", "HEAD"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_core::types::new_id;

    struct Fixture {
        home: tempfile::TempDir,
        repo: tempfile::TempDir,
    }

    impl Fixture {
        /// Main checkout: a committed file on a baseline branch, with a
        /// tracked modification and an untracked file left DIRTY on purpose
        /// — the dirty state is the thing isolation must keep out of the
        /// worktree, and main's `status` is the before/after witness for
        /// REQ-WT-08 AC2.
        fn new() -> Fixture {
            let home = tempfile::TempDir::new().unwrap();
            let repo = tempfile::TempDir::new().unwrap();
            let p = repo.path();
            git(p, &["init", "-q"]).unwrap();
            std::fs::write(p.join("base.txt"), b"baseline\n").unwrap();
            git(p, &["add", "base.txt"]).unwrap();
            git(p, &["-c", "user.email=devai@test.local", "-c", "user.name=devai-test", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "bound state"]).unwrap();
            std::fs::write(p.join("base.txt"), b"dirty user edit\n").unwrap();
            std::fs::write(p.join("loose.txt"), b"untracked\n").unwrap();
            Fixture { home, repo }
        }
        fn head(&self) -> String {
            git(self.repo.path(), &["rev-parse", "HEAD"]).unwrap()
        }
        fn main_state(&self) -> (String, String, String) {
            (
                git(self.repo.path(), &["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"]).unwrap(),
                git(self.repo.path(), &["rev-parse", "HEAD"]).unwrap(),
                git(self.repo.path(), &["ls-files", "-s"]).unwrap(),
            )
        }
    }

    #[test]
    fn branch_shape_is_the_business_spelling() {
        let id = new_id();
        assert_eq!(branch_name(id), format!("rework/{id}"));
        assert!(path_for(Path::new("/home/u/.devai"), id).starts_with("/home/u/.devai/worktrees/"));
    }

    #[test]
    fn prepare_creates_branch_at_the_bound_commit_inside_home() {
        let f = Fixture::new();
        let id = new_id();
        let wt = prepare(f.repo.path(), f.home.path(), id, Some(&f.head())).unwrap();
        assert_eq!(wt.branch, branch_name(id));
        assert_eq!(current_branch(&wt.dir).as_deref(), Some(wt.branch.as_str()));
        // Branch points at exactly the bound HEAD (REQ-WT-01 AC1).
        assert_eq!(git(&wt.dir, &["rev-parse", "HEAD"]).unwrap(), f.head());
        // The worktree carries the COMMITTED tree — not main's dirty state.
        assert_eq!(std::fs::read_to_string(wt.dir.join("base.txt")).unwrap(), "baseline\n");
        assert!(!wt.dir.join("loose.txt").exists());
    }

    #[test]
    fn writing_stage_never_touches_the_main_working_tree() {
        let f = Fixture::new();
        let id = new_id();
        let before = f.main_state();
        let baseline_ref = git(f.repo.path(), &["rev-parse", "HEAD"]).unwrap();
        let wt = prepare(f.repo.path(), f.home.path(), id, Some(&f.head())).unwrap();
        std::fs::write(wt.dir.join("new.txt"), b"agent work\n").unwrap();
        let sha = commit_platform_work(&wt.dir, "devai: implementation: demo", id, new_id()).unwrap();
        let after = f.main_state();
        assert_eq!(after, before, "status/HEAD/index of the main checkout must be byte-identical across a full writing stage (REQ-WT-08 AC2)");
        assert_eq!(git(f.repo.path(), &["rev-parse", "HEAD"]).unwrap(), baseline_ref, "HEAD unmoved");
        // The only ref that moved is the task's own branch.
        assert_eq!(git(f.repo.path(), &["rev-parse", &branch_name(id)]).unwrap(), sha);
        // Nothing of the run leaked into the main checkout's tree.
        assert!(!f.repo.path().join("worktrees").exists());
        assert!(!f.repo.path().join("evidence").exists());
    }

    #[test]
    fn commit_with_staged_content_returns_tip_and_renames_nothing() {
        let f = Fixture::new();
        let id = new_id();
        let wt = prepare(f.repo.path(), f.home.path(), id, Some(&f.head())).unwrap();
        let tip = commit_platform_work(&wt.dir, "devai: implementation: nothing changed", id, new_id()).unwrap();
        assert_eq!(tip, f.head(), "zero-change writing run names the capturing tip, not an empty commit");
        let count = git(&wt.dir, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count, "1");
    }

    #[test]
    fn no_bound_commit_is_an_explicit_failure_not_a_fallback() {
        let f = Fixture::new();
        let err = prepare(f.repo.path(), f.home.path(), new_id(), None).unwrap_err();
        assert!(matches!(err, WorktreeError::NoBoundCommit), "{err}");
        assert!(err.to_string().contains("fails explicitly"), "the AC2 rule is in the message itself");
    }

    #[test]
    fn non_git_workspace_is_refused() {
        let home = tempfile::TempDir::new().unwrap();
        let nongit = tempfile::TempDir::new().unwrap();
        let err = prepare(nongit.path(), home.path(), new_id(), Some("deadbeef")).unwrap_err();
        assert!(matches!(err, WorktreeError::NotGit(_)), "{err}");
    }

    #[test]
    fn rework_reuses_the_branch_after_the_directory_is_gone() {
        let f = Fixture::new();
        let id = new_id();
        let wt = prepare(f.repo.path(), f.home.path(), id, Some(&f.head())).unwrap();
        std::fs::write(wt.dir.join("wip.txt"), b"first run\n").unwrap();
        let sha1 = commit_platform_work(&wt.dir, "devai: implementation: first", id, new_id()).unwrap();
        // A human deleted the directory (branch ref persists — REQ-WT-07 AC3).
        std::fs::remove_dir_all(&wt.dir).unwrap();
        let wt2 = prepare(f.repo.path(), f.home.path(), id, Some(&f.head())).unwrap();
        assert_eq!(wt2.dir, wt.dir);
        assert_eq!(current_branch(&wt2.dir).as_deref(), Some(branch_name(id).as_str()));
        // Reuse attaches the EXISTING branch — not a fresh one at the bound
        // HEAD: the first run's committed work continues.
        assert_eq!(git(&wt2.dir, &["rev-parse", "HEAD"]).unwrap(), sha1);
        assert_eq!(std::fs::read_to_string(wt2.dir.join("wip.txt")).unwrap(), "first run\n");
    }

    #[test]
    fn foreign_tree_in_the_worktree_slot_is_a_mismatch() {
        let f = Fixture::new();
        let id = new_id();
        std::fs::create_dir_all(path_for(f.home.path(), id)).unwrap();
        let err = prepare(f.repo.path(), f.home.path(), id, Some(&f.head())).unwrap_err();
        assert!(matches!(err, WorktreeError::BranchMismatch { .. }), "{err}");
    }
}
