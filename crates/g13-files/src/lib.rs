//! Writing a file so that a reader never sees half of it.
//!
//! One rule, in one place. A file that another process reads while it is being replaced is written beside
//! itself and renamed into place, so a reader sees all of one version or all of the other and never a
//! truncated one. A plain write truncates first, and what reads these files is a renderer redrawing a screen
//! twenty times a second or a driver reloading a map when its stamp changes - which is how a half-written
//! file becomes a screen that says nothing, once in a while, with nothing in any log to explain it.
//!
//! `write` is the ordinary case. `write_owner_only` is the same thing for a file that holds a credential,
//! which is also created with its mode rather than corrected afterwards: a token file that is readable for
//! the moment between the write and a `chmod` is a token given away.
//!
//! This is the only place either of those is done. Every writer of a configuration file calls these, and the
//! before-and-after of a rename is tested here rather than in each of them.

// a test may unwrap and may fail loudly: a test that cannot panic on a fixture cannot fail
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::print_stdout,
        clippy::print_stderr
    )
)]
use std::io::Write;
use std::path::Path;

/// Write `text` to `path`, beside itself and then into place.
pub fn write(path: &Path, text: &str) -> Result<(), String> {
    write_with_mode(path, text, false)
}

/// Write `text` to `path` so that only its owner can read it, the same way round.
pub fn write_owner_only(path: &Path, text: &str) -> Result<(), String> {
    write_with_mode(path, text, true)
}

/// The one implementation. The two differ in the mode and in nothing else, and two copies of this would
/// drift into one of them forgetting the rename, which is the fault this module exists to remove.
fn write_with_mode(path: &Path, text: &str, owner_only: bool) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{} could not be created: {error}", parent.display()))?;
    }
    let beside = beside(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        if owner_only {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
    }
    #[cfg(not(unix))]
    let _ = owner_only;
    let mut file = options
        .open(&beside)
        .map_err(|error| format!("{} could not be written: {error}", beside.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("{} could not be written: {error}", beside.display()))?;
    #[cfg(unix)]
    {
        if owner_only {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| {
                    format!(
                        "{} could not be made the owner's alone: {error}",
                        beside.display()
                    )
                })?;
        }
    }
    drop(file);
    // the whole of the rename: a reader opened before this sees the old file, one opened after sees the new
    std::fs::rename(&beside, path)
        .map_err(|error| format!("{} could not be put in place: {error}", path.display()))
}

/// The name the half-written file goes under: the whole name with `.writing` after it, so
/// `endpoints.json.writing` and `bindings-0.properties.writing` both say what they are. A suffix rather than
/// a replaced extension, because a replaced one would turn `menu.json` into `menu.writing` and two files
/// written at once would collide.
fn beside(path: &Path) -> std::path::PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".writing");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("g13-files-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_reader_never_sees_half_a_file() {
        // The fault this is here for: a plain write truncates first, so a reader that opens the file in that
        // window sees a prefix of it, or nothing at all. The reader below is that reader - it reads the file
        // over and over while it is being replaced, and every read has to be all of one version or all of the
        // other. The two versions are long enough that a truncating write is caught rather than raced past.
        let dir = dir("half");
        let path = dir.join("bindings-0.properties");
        let first = "G1=p,k.30\n".repeat(20_000);
        let second = "G2=p,k.31\n".repeat(20_000);
        write(&path, &first).expect("the first write");

        let watched = path.clone();
        let reader = std::thread::spawn(move || {
            let mut seen = 0usize;
            for _ in 0..400 {
                let Ok(text) = std::fs::read_to_string(&watched) else {
                    continue;
                };
                assert!(
                    text == "G1=p,k.30\n".repeat(20_000) || text == "G2=p,k.31\n".repeat(20_000),
                    "a reader saw {} bytes that are neither version - a partial file",
                    text.len()
                );
                seen += 1;
            }
            seen
        });
        for round in 0..40 {
            match round % 2 {
                0 => write(&path, &second).expect("a write"),
                _ => write(&path, &first).expect("a write"),
            }
        }
        let seen = reader.join().expect("the reader");
        assert!(seen > 100, "the reader only managed {seen} reads");
        assert!(
            !dir.join("bindings-0.properties.writing").exists(),
            "the half-written file is still there"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_owner_only_file_is_the_owners_alone_and_a_plain_one_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let mode_of =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir = dir("mode");
        let secret = dir.join("endpoints.json");
        let plain = dir.join("menu.json");
        write_owner_only(&secret, "{\"token\": \"x\"}").expect("written");
        write(&plain, "{}").expect("written");
        assert_eq!(mode_of(&secret), 0o600, "a credential is the owner's alone");
        assert_ne!(mode_of(&plain), 0o600, "and an ordinary file is not");

        // and a save over a file that was readable by everybody makes it the owner's alone again, because the
        // mode travels with whichever file is renamed into place
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_owner_only(&secret, "{\"token\": \"y\"}").expect("written again");
        assert_eq!(mode_of(&secret), 0o600);
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "{\"token\": \"y\"}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_there_yet_is_created_with_its_directory() {
        let dir = dir("nested");
        let path = dir.join("a/b/c.json");
        write(&path, "{}\n").expect("written");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
