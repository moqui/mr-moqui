/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The targets of `moqui test-regression`: which engine, and how it is built. A build is kept by commit under
//! `target/test-regression/` of the repository, so a second run of the same commit does not build again.

use std::path::{Path, PathBuf};
use std::process::Command;

use moqui_lib::error::Error;

/// An engine to run, as the command line named it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// `rust:.`: this binary and its runtime.
    RustHere,
    /// `rust:<ref>`: mr-moqui at that commit, built.
    RustRef(String),
    /// `java:.` is the Java checkout as it is (its `moqui.war` and `runtime`); `java:<ref>` is the framework at that
    /// commit, built, with the checkout's runtime.
    Java { reference: String, repo: PathBuf },
}

/// A target ready to start: the program and the runtime it copies.
#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    /// The target as it was named, with its commit, for the report.
    pub name: String,
    pub kind: Kind,
    /// The runtime to copy for the run.
    pub runtime: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    Rust { binary: PathBuf },
    Java { war: PathBuf },
}

impl Target {
    pub(crate) fn parse(text: &str, java_repo: Option<&Path>) -> Result<Self, Error> {
        let (engine, reference) = text
            .split_once(':')
            .ok_or_else(|| Error::new(format!("target {text}: give rust:<ref>, rust:. or java:<ref>")))?;
        if reference.is_empty() {
            return Err(Error::new(format!("target {text}: no commit after the colon")));
        }
        match engine {
            "rust" if reference == "." => Ok(Self::RustHere),
            "rust" => Ok(Self::RustRef(reference.to_string())),
            "java" => {
                let repo = java_repo.ok_or_else(|| {
                    Error::new(format!(
                        "target {text}: give the Moqui Java checkout with --java-repo or MOQUI_JAVA_REPO"
                    ))
                })?;
                Ok(Self::Java {
                    reference: reference.to_string(),
                    repo: repo.to_path_buf(),
                })
            }
            other => Err(Error::new(format!("target {text}: {other} is not rust or java"))),
        }
    }

    pub(crate) fn is_java(&self) -> bool {
        matches!(self, Self::Java { .. })
    }

    pub(crate) fn name(&self) -> String {
        match self {
            Self::RustHere => "rust:.".to_string(),
            Self::RustRef(reference) => format!("rust:{reference}"),
            Self::Java { reference, .. } => format!("java:{reference}"),
        }
    }

    /// Build the target when it is not built yet.
    pub(crate) fn prepare(&self, runtime: &Path, out: &Path, print: &mut dyn FnMut(&str)) -> Result<Prepared, Error> {
        match self {
            Self::RustHere => {
                let binary = std::env::current_exe()
                    .map_err(|err| Error::new("could not find this binary").with_source(err))?;
                let repo = runtime.parent().unwrap_or(runtime);
                Ok(Prepared {
                    name: format!("rust:. ({})", describe(repo)),
                    kind: Kind::Rust { binary },
                    runtime: runtime.to_path_buf(),
                })
            }
            Self::RustRef(reference) => {
                let repo = git_top(runtime.parent().unwrap_or(runtime))?;
                let sha = resolve(&repo, reference)?;
                let cache = repo.join("target/test-regression").join(format!("rust-{}", &sha[..12]));
                let binary = cache.join(binary_name());
                let src = cache.join("src");
                if !binary.is_file() {
                    print(&format!("building rust:{reference} ({}) in {}", &sha[..12], src.display()));
                    checkout(&repo, &sha, &src)?;
                    let target_dir = repo.join("target/test-regression/cargo");
                    let log = cache.join("build.log");
                    let mut command = Command::new(cargo());
                    command
                        .args(["build", "-p", "moqui", "--bin", "moqui"])
                        .current_dir(&src)
                        .env("CARGO_TARGET_DIR", &target_dir);
                    run_logged(command, &log, &format!("the build of rust:{reference}"))?;
                    std::fs::copy(target_dir.join("debug").join(binary_name()), &binary).map_err(|err| {
                        Error::new(format!("could not copy the build of rust:{reference}")).with_source(err)
                    })?;
                }
                Ok(Prepared {
                    name: format!("rust:{reference} ({})", &sha[..12]),
                    kind: Kind::Rust { binary },
                    runtime: src.join("runtime"),
                })
            }
            Self::Java { reference, repo } => {
                let runtime = repo.join("runtime");
                if !runtime.join("conf").is_dir() {
                    return Err(Error::new(format!(
                        "java:{reference}: {} has no runtime (moqui-runtime cloned into runtime/)",
                        repo.display()
                    )));
                }
                if reference == "." {
                    let war = repo.join("moqui.war");
                    if !war.is_file() {
                        return Err(Error::new(format!(
                            "java:.: {} has no moqui.war; build it (./gradlew build) or name a commit",
                            repo.display()
                        )));
                    }
                    return Ok(Prepared {
                        name: format!("java:. ({})", describe(repo)),
                        kind: Kind::Java { war },
                        runtime,
                    });
                }
                let sha = resolve(repo, reference)?;
                let own = git_top(runtime.parent().unwrap_or(runtime.as_path()))
                    .unwrap_or_else(|_| out.to_path_buf())
                    .join("target/test-regression");
                let cache = own.join(format!("java-{}", &sha[..12]));
                let war = cache.join("moqui.war");
                if !war.is_file() {
                    let src = cache.join("src");
                    print(&format!("building java:{reference} ({}) in {}", &sha[..12], src.display()));
                    checkout(repo, &sha, &src)?;
                    // the build reads the runtime's components; it is the checkout's, not of the commit
                    let link = src.join("runtime");
                    if !link.exists() {
                        symlink_dir(&runtime, &link)?;
                    }
                    let mut command = Command::new(src.join("gradlew"));
                    command.args(["--no-daemon", "build", "-x", "test"]).current_dir(&src);
                    run_logged(command, &cache.join("build.log"), &format!("the build of java:{reference}"))?;
                    std::fs::copy(src.join("moqui.war"), &war).map_err(|err| {
                        Error::new(format!("could not copy the moqui.war of java:{reference}")).with_source(err)
                    })?;
                }
                Ok(Prepared {
                    name: format!("java:{reference} ({})", &sha[..12]),
                    kind: Kind::Java { war },
                    runtime,
                })
            }
        }
    }
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "moqui.exe"
    } else {
        "moqui"
    }
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn git(repo: &Path, args: &[&str]) -> Result<String, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|err| Error::new("could not run git").with_source(err))?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "git {} in {}: {}",
            args.join(" "),
            repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_top(dir: &Path) -> Result<PathBuf, Error> {
    git(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn resolve(repo: &Path, reference: &str) -> Result<String, Error> {
    git(repo, &["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
}

/// The commit of `repo` and whether its tree has changes, such as `4333d16a6c1f +changes`.
fn describe(repo: &Path) -> String {
    let sha = git(repo, &["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|_| "no git".to_string());
    let dirty = git(repo, &["status", "--porcelain", "--untracked-files=no"]).is_ok_and(|text| !text.is_empty());
    if dirty {
        format!("{sha} +changes")
    } else {
        sha
    }
}

/// A detached worktree of `sha` at `dir`, made again when it is at another commit.
fn checkout(repo: &Path, sha: &str, dir: &Path) -> Result<(), Error> {
    if dir.join(".git").exists() {
        if git(dir, &["rev-parse", "HEAD"]).is_ok_and(|head| head == sha) {
            return Ok(());
        }
        git(repo, &["worktree", "remove", "--force", &dir.display().to_string()])?;
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::new(format!("could not create {}", parent.display())).with_source(err))?;
    }
    git(repo, &["worktree", "add", "--detach", "--force", &dir.display().to_string(), sha]).map(|_| ())
}

fn run_logged(mut command: Command, log: &Path, what: &str) -> Result<(), Error> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).map_err(|err| Error::new(format!("could not create {}", dir.display())).with_source(err))?;
    }
    let file = std::fs::File::create(log).map_err(|err| Error::new(format!("could not create {}", log.display())).with_source(err))?;
    let err_file = file
        .try_clone()
        .map_err(|err| Error::new(format!("could not open {}", log.display())).with_source(err))?;
    let status = command
        .stdout(file)
        .stderr(err_file)
        .status()
        .map_err(|err| Error::new(format!("{what} did not start")).with_source(err))?;
    if !status.success() {
        return Err(Error::new(format!("{what} failed ({status}); see {}", log.display())));
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> Result<(), Error> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|err| Error::new(format!("could not link {}", link.display())).with_source(err))
}

#[cfg(not(unix))]
fn symlink_dir(target: &Path, link: &Path) -> Result<(), Error> {
    std::os::windows::fs::symlink_dir(target, link)
        .map_err(|err| Error::new(format!("could not link {}", link.display())).with_source(err))
}
