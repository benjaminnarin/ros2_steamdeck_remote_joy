//! Embeds the build's git commit for `--version` and the Menu screen: DECK_BUILD =
//! "<short hash>[-dirty] <commit date>", or "unknown" outside a git checkout.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn main() {
    let build = match (git(&["rev-parse", "--short=7", "HEAD"]), git(&["log", "-1", "--format=%cs"])) {
        (Some(hash), Some(date)) => {
            // Uncommitted changes (tracked files) make it "-dirty".
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
            format!("{hash}{} {date}", if dirty { "-dirty" } else { "" })
        }
        _ => "unknown".to_string(),
    };
    println!("cargo:rustc-env=DECK_BUILD={build}");
    // Re-run when the commit or the working tree changes.
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/index");
        if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={dir}/{r}");
        }
    }
}
