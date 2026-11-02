//! Workspace revision: the deterministic, platform-side fingerprint of the
//! exact state an execution ran against. Computed by `git` plumbing + a
//! standard hash — NEVER by a model (sovereignty: the number is evidence,
//! not judgment). Observation only: every command here is read-only
//! (`status`, `ls-files`, `hash-object`), so capturing a revision cannot
//! change the execution outcome or the working tree.

use std::path::Path;
use std::process::Command;

use devai_core::types::WorkspaceRevision;
use devai_store::config::Profile;
use sha2::{Digest, Sha256};

/// SHA-256 hex of the empty digest input — the canonical value for a clean
/// tree, distinct from any non-empty dirty sequence.
const CLEAN_TREE_DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Build the full four-component revision. `roles` is the list of current
/// role definitions as `slug@version` (the workspace's role set at execution
/// time); the engine owns how it is gathered.
pub fn compute(workspace: &Path, profile: Option<&Profile>, roles: &[String]) -> WorkspaceRevision {
    let (git_head, dirty_digest) = tree_state(workspace);
    WorkspaceRevision {
        git_head,
        dirty_digest,
        profile: profile.map(|p| p.identity()),
        roles: roles.to_vec(),
    }
}

/// `git rev-parse HEAD` of the workspace, trimmed. None when the directory is
/// not a git repo or git is unavailable — such a revision is not comparable
/// and freshness must never be claimed from it.
pub fn git_head(workspace: &Path) -> Option<String> {
    let out = Command::new("git").arg("rev-parse").arg("HEAD").current_dir(workspace).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Canonical dirty-tree digest. Returns (head, digest); both are None when
/// the workspace is not a git repo. A git repo with a clean tree returns
/// (head, Some(CLEAN_TREE_DIGEST)).
fn tree_state(workspace: &Path) -> (Option<String>, Option<String>) {
    let head = git_head(workspace);
    // Enumerate every path that differs from HEAD or is untracked-non-ignored.
    // --no-renames keeps each NUL record 1:1 (a rename would otherwise append
    // an extra origin path); --untracked-files=all lists every loose file.
    let porcelain = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(workspace)
        .output();
    let porcelain = match porcelain {
        Ok(o) if o.status.success() => o.stdout,
        _ => return (head, None),
    };

    let mut entries: Vec<String> = Vec::new();
    for record in porcelain.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        // record layout: XY SP path   (XY = 2 status bytes, SP = 0x20)
        if record.len() < 4 {
            continue;
        }
        let xy = String::from_utf8_lossy(&record[0..2]).into_owned();
        let path = &record[3..];
        let path = String::from_utf8_lossy(path).into_owned();
        entries.push(format!("{xy}\t{wt_oid}\t{idx_oid}\t{path}", wt_oid = worktree_oid(workspace, &path), idx_oid = index_oid(workspace, &path)));
    }

    // Order-independent: sort the canonical sequence, hash it.
    entries.sort();
    let digest = if entries.is_empty() {
        CLEAN_TREE_DIGEST.to_string()
    } else {
        let joined = entries.join("\n");
        hex(&Sha256::digest(joined.as_bytes()))
    };
    (head, Some(digest))
}

/// `git hash-object` of the worktree file (read-only, no `-w`). `-` for a
/// deleted or unreadable path so a removal still shifts the digest.
fn worktree_oid(workspace: &Path, path: &str) -> String {
    let full = workspace.join(path);
    if !full.exists() {
        return "-".into();
    }
    // hash-object with -- allows a leading-dash path safely.
    match Command::new("git").args(["hash-object", "--"]).arg(path).current_dir(workspace).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "-".into(),
    }
}

/// Staged (index) blob oid for the path, `-` when the path is untracked or
/// absent from the index. `git ls-files -s` lists the index entry (stage 0).
fn index_oid(workspace: &Path, path: &str) -> String {
    match Command::new("git").args(["ls-files", "-s", "--"]).arg(path).current_dir(workspace).output() {
        Ok(o) if o.status.success() => {
            let line = String::from_utf8_lossy(&o.stdout);
            // "mode oid stage\tpath" -> oid is the second whitespace token.
            line.split_whitespace().nth(1).map(|s| s.to_string()).unwrap_or_else(|| "-".into())
        }
        _ => "-".into(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Repo {
        dir: tempfile::TempDir,
    }

    impl Repo {
        /// Fresh git repo with one committed file; git identity pinned so
        /// the test never depends on the machine's global config.
        fn new() -> Repo {
            let dir = tempfile::TempDir::new().unwrap();
            git(dir.path(), &["init", "-q"]);
            std::fs::write(dir.path().join("a.txt"), b"alpha\n").unwrap();
            git(dir.path(), &["add", "a.txt"]);
            git(
                dir.path(),
                &["-c", "user.email=devai@test.local", "-c", "user.name=devai-test", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"],
            );
            Repo { dir }
        }
        fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git is available");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn digest(r: &Repo) -> Option<String> {
        compute(r.path(), None, &[]).dirty_digest
    }

    #[test]
    fn clean_tree_has_a_defined_distinct_constant() {
        let r = Repo::new();
        let rev = compute(r.path(), None, &[]);
        assert!(rev.git_head.is_some(), "committed repo has HEAD");
        assert_eq!(rev.dirty_digest.as_deref(), Some(CLEAN_TREE_DIGEST), "clean tree → empty-sequence constant");
        assert!(rev.is_comparable());
    }

    #[test]
    fn digest_is_deterministic_on_an_unchanged_dirty_tree() {
        let r = Repo::new();
        std::fs::write(r.path().join("a.txt"), b"changed\n").unwrap();
        std::fs::write(r.path().join("loose.txt"), b"untracked\n").unwrap();
        assert_eq!(digest(&r), digest(&r), "two computations on the same state must agree");
        assert_ne!(digest(&r).as_deref(), Some(CLEAN_TREE_DIGEST));
    }

    #[test]
    fn every_dirty_dimension_moves_the_digest() {
        let r = Repo::new();
        let clean = digest(&r);

        // tracked modification
        std::fs::write(r.path().join("a.txt"), b"beta\n").unwrap();
        let modified = digest(&r);
        assert_ne!(modified, clean, "tracked edit changes the digest");

        // staged content differs from worktree → staging the same edit moves it again
        git(r.path(), &["add", "a.txt"]);
        let staged = digest(&r);
        assert_ne!(staged, modified, "index change with the same worktree moves the digest");

        // untracked file
        std::fs::write(r.path().join("new.txt"), b"x\n").unwrap();
        let with_untracked = digest(&r);
        assert_ne!(with_untracked, staged, "untracked non-ignored file changes the digest");

        // removing the untracked file returns to the previous digest
        std::fs::remove_file(r.path().join("new.txt")).unwrap();
        assert_eq!(digest(&r), staged, "digest tracks removal as precisely as addition");

        // deletion of a tracked file changes the digest (worktree oid '-')
        std::fs::remove_file(r.path().join("a.txt")).unwrap();
        assert_ne!(digest(&r), staged, "deleted tracked file must not collide with its prior states");
    }

    #[test]
    fn ignored_files_do_not_affect_the_digest() {
        let r = Repo::new();
        // Commit an ignore rule, then return to a clean state.
        std::fs::write(r.path().join(".gitignore"), b"ignored/\n").unwrap();
        git(r.path(), &["add", ".gitignore"]);
        git(
            r.path(),
            &["-c", "user.email=devai@test.local", "-c", "user.name=devai-test", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "ignore rule"],
        );
        std::fs::create_dir(r.path().join("ignored")).unwrap();
        let clean = digest(&r);
        assert_eq!(clean.as_deref(), Some(CLEAN_TREE_DIGEST), "committed .gitignore leaves the tree clean");

        // Files inside the ignored directory must not move the digest.
        std::fs::write(r.path().join("ignored/junk.txt"), b"junk\n").unwrap();
        std::fs::write(r.path().join("ignored/more.txt"), b"more\n").unwrap();
        assert_eq!(digest(&r), clean, "ignored files do not affect the digest");

        // While non-ignored untracked files DO: the contrast must be real,
        // not an accident of everything being ignored.
        std::fs::write(r.path().join("visible.txt"), b"seen\n").unwrap();
        assert_ne!(digest(&r), clean, "non-ignored untracked file moves the digest");
    }

    #[test]
    fn order_of_paths_does_not_change_the_digest() {
        // The canonical sequence is sorted internally: two repos with the
        // same final state reached in different orders must agree.
        let r1 = Repo::new();
        std::fs::write(r1.path().join("z.txt"), b"1\n").unwrap();
        std::fs::write(r1.path().join("a_untracked.txt"), b"2\n").unwrap();
        let d1 = digest(&r1);

        let r2 = Repo::new();
        std::fs::write(r2.path().join("a_untracked.txt"), b"2\n").unwrap();
        std::fs::write(r2.path().join("z.txt"), b"1\n").unwrap();
        assert_eq!(d1, digest(&r2), "creation order must not affect the digest");
    }

    #[test]
    fn computation_is_read_only() {
        let r = Repo::new();
        std::fs::write(r.path().join("a.txt"), b"dirty\n").unwrap();
        std::fs::write(r.path().join("loose.txt"), b"untracked\n").unwrap();
        let status_before = git(r.path(), &["status", "--porcelain"]);
        let index_before = git(r.path(), &["ls-files", "-s"]);
        let _ = compute(r.path(), None, &[]);
        let _ = compute(r.path(), None, &[]);
        // hash-object without -w never writes a blob; status/ls-files are
        // plumbing reads. Neither the worktree, the index, nor HEAD may move.
        assert_eq!(git(r.path(), &["status", "--porcelain"]), status_before, "computing the digest must not change the tree state");
        assert_eq!(git(r.path(), &["ls-files", "-s"]), index_before, "computing the digest must not touch the index");
    }

    #[test]
    fn non_git_workspace_records_null_head_and_digest() {
        let dir = tempfile::TempDir::new().unwrap();
        let rev = compute(dir.path(), None, &[]);
        assert_eq!(rev.git_head, None);
        assert_eq!(rev.dirty_digest, None);
        assert!(!rev.is_comparable(), "a non-git revision must never support a freshness claim");
    }

    #[test]
    fn profile_and_roles_components_are_carried_verbatim() {
        // The revision is a pure carrier for the platform-supplied identity:
        // profile name@version and sorted slug@version lists pass through.
        let r = Repo::new();
        let rev = compute(
            r.path(),
            None,
            &["quality-auditor@2".to_string(), "requirements-analyst@1".to_string()],
        );
        assert_eq!(rev.profile, None, "no profile loaded → recorded as absent, not guessed");
        assert_eq!(rev.roles, vec!["quality-auditor@2".to_string(), "requirements-analyst@1".to_string()]);
    }
}
