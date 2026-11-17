//! Crash-safe writes: the data goes to a temp file in the target's directory, is synced to
//! disk, then renamed into place, so readers see the old file or the new one, never a mix.

use std::fs::{self, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tempfile::NamedTempFile;

/// Temp files start with a dot, so `Journal::list` never shows a leftover one.
const TEMP_PREFIX: &str = ".mdgj-tmp-";

/// Replaces (or creates) `path` with `bytes`, keeping the permissions of the file it replaces.
pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let permissions = match fs::metadata(path) {
        Ok(meta) => Some(meta.permissions()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    synced_temp(dir, bytes, permissions)?.persist(path)?;
    Ok(())
}

/// Writes `bytes` to the first of `dir/name(1)`, `dir/name(2)`, … that doesn't exist yet, and
/// returns that name. Never replaces an existing file.
pub(crate) fn write_new(
    dir: &Path,
    bytes: &[u8],
    name: impl Fn(u32) -> String,
) -> io::Result<String> {
    let mut file = synced_temp(dir, bytes, None)?;
    let mut n = 1;
    loop {
        let candidate = name(n);
        match file.persist_noclobber(dir.join(&candidate)) {
            Ok(_) => return Ok(candidate),
            Err(err) if err.error.kind() == io::ErrorKind::AlreadyExists => file = err.file,
            Err(err) => return Err(err.error),
        }
        n += 1;
    }
}

/// A temp file in `dir` holding `bytes`, with exactly `permissions` if given, or else those
/// `File::create` gives (temp files default to 0600).
fn synced_temp(
    dir: &Path,
    bytes: &[u8],
    permissions: Option<Permissions>,
) -> io::Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        // Filtered by the umask, like `File::create`.
        .permissions(Permissions::from_mode(0o666))
        .tempfile_in(dir)?;
    if let Some(permissions) = permissions {
        // Unlike the mode at creation, not filtered by the umask.
        file.as_file().set_permissions(permissions)?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    Ok(file)
}
