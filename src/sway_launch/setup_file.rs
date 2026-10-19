//! `--cwd`/`--env` kept off the launch shell's command line (#167).
//!
//! Sway starts a launch as `sh -c "<exec line>"`, and that shell stays alive
//! as the program's parent for as long as the program runs (see
//! `ExecAction::sway_command()`), so anything in the exec line is readable by
//! every local user through `/proc/<pid>/cmdline` for the program's lifetime.
//! A variable's value is often a token or a password, so the rendered setup
//! goes into a file only its owner can read, and the exec line names only the
//! file: the launch shell sources it and removes it.

use std::ffi::OsStr;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

/// One launch's setup file, removed when this is dropped: when the launch
/// shell never got as far as removing it itself (a failed `cd`, a timeout, an
/// interrupted run), or harmlessly after it did. A process killed by a signal
/// it doesn't handle leaves the file behind, readable by its owner only.
#[derive(Debug)]
pub(super) struct SetupFile {
    path: PathBuf,
}

impl SetupFile {
    /// Writes `setup` for the launch whose marker is `token`, under
    /// `$XDG_RUNTIME_DIR/sway-launch/`. `None` when there is no runtime
    /// directory: the setup then goes on the command line as before, with a
    /// warning, once per process, that its values are visible there.
    pub(super) fn write(setup: &str, token: &str) -> Result<Option<SetupFile>, String> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
        let Some(directory) = setup_file_directory(runtime_dir.as_deref()) else {
            warn_setup_on_the_command_line();
            return Ok(None);
        };
        SetupFile::write_in(&directory, setup, token).map(Some)
    }

    /// `write()` into `directory`, created (mode 0700) when missing. The file
    /// is created new, mode 0600, so it never exists readable by anyone else.
    pub(super) fn write_in(
        directory: &Path,
        setup: &str,
        token: &str,
    ) -> Result<SetupFile, String> {
        let failure = |path: &Path, error: std::io::Error| {
            format!("--cwd/--env setup file {}: {}", path.display(), error)
        };
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| failure(directory, error))?;
        let path = directory.join(format!("launch-{}", token));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| failure(&path, error))?;
        let setup_file = SetupFile { path };
        file.write_all(setup_file_contents(setup).as_bytes())
            .map_err(|error| failure(&setup_file.path, error))?;
        Ok(setup_file)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SetupFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Where setup files go: `$XDG_RUNTIME_DIR/sway-launch/`, beside the run
/// locks, or `None` when the variable is unset or empty.
fn setup_file_directory(runtime_dir: Option<&OsStr>) -> Option<PathBuf> {
    runtime_dir
        .filter(|directory| !directory.is_empty())
        .map(|directory| Path::new(directory).join("sway-launch"))
}

/// The file's text: the setup exactly as it would have gone into the exec
/// line. `render_launch_setup()` quotes for Sway's parser and then `sh`, and
/// a word quoted that way reads the same to `sh` alone: Sway passes the `\\`
/// it writes outside the quotes through unchanged, so a file read straight by
/// `sh` sees the same text the launch shell used to.
fn setup_file_contents(setup: &str) -> String {
    format!("{}\n", setup)
}

fn warn_setup_on_the_command_line() {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        diagnostic!(
            "{}XDG_RUNTIME_DIR is not set, so --cwd/--env go on the launch's command line, \
             where every local user can read them while the program runs",
            crate::stderr_label(crate::Severity::Warning)
        );
    });
}

#[cfg(test)]
mod tests;
