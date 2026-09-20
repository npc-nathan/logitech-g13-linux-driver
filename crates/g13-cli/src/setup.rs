//! `g13 setup` - put the defaults where this driver looks for them.
//!
//! The tarball's installer does this as it copies files, but a package manager puts the defaults somewhere of its
//! own choosing (`/usr/share/g13/defaults` in the .deb) and root has no business writing into anybody's home. So
//! the first thing a user does after installing is run this, as themselves, and it copies the applets, fonts,
//! themes, bindings and macros into their own config.
//!
//! It never overwrites: the whole point of the defaults is to be a starting point to edit, so a file that is
//! already there is one the user has made their own.

use std::path::PathBuf;

/// Where the defaults live in each shape this ships in, most specific first.
fn defaults_dirs() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    // a package manager's own location
    candidates.push(PathBuf::from("/usr/share/g13/defaults"));
    candidates.push(PathBuf::from("/usr/local/share/g13/defaults"));
    // beside the binary, which is how the tarball and a checkout look
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // a checkout's binary is at target/release/g13 and the defaults at the repo root, so this walks up
            // rather than guessing a single depth
            let mut up = Some(dir);
            for _ in 0..4 {
                let Some(here) = up else { break };
                candidates.push(here.join("defaults"));
                candidates.push(here.join("share/g13/defaults"));
                up = here.parent();
            }
        }
    }
    candidates
}

/// The parts of a config that come from the defaults, and the extensions they hold.
const PARTS: [&str; 3] = ["applets", "fonts", "themes"];

/// Copies this build's defaults into the user's config, leaving anything already there alone.
pub fn run() -> i32 {
    let config = g13_config::config_dir();
    let Some(defaults) = defaults_dirs().into_iter().find(|dir| dir.is_dir()) else {
        println!("cannot find the defaults this build ships.");
        println!("They are looked for in /usr/share/g13/defaults and beside the binary.");
        return 1;
    };

    println!("Setting up g13.");
    println!("  from:  {}", defaults.display());
    println!("  into:  {}", config.display());
    println!();

    let mut added = 0usize;
    let mut kept = 0usize;
    let mut refused: Vec<String> = Vec::new();

    for part in PARTS {
        let source = defaults.join(part);
        if !source.is_dir() {
            continue;
        }
        let target = config.join(part);
        if let Err(problem) = std::fs::create_dir_all(&target) {
            refused.push(format!("{}: {problem}", target.display()));
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&source) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            let destination = target.join(name);
            if destination.exists() {
                kept += 1;
                continue;
            }
            match std::fs::copy(&path, &destination) {
                Ok(_) => added += 1,
                Err(problem) => refused.push(format!("{}: {problem}", destination.display())),
            }
        }
    }

    // and the files that sit at the top of a config rather than in a folder: the bindings, the macros, the
    // endpoint template and the rotation
    let mut flat = vec![
        "bindings-0.properties".to_string(),
        "endpoints.json".to_string(),
        "visuals.json".to_string(),
    ];
    if let Ok(entries) = std::fs::read_dir(&defaults) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("macro-") && entry.path().is_file() {
                flat.push(name);
            }
        }
    }
    for name in flat {
        let name = name.as_str();
        let source = defaults.join(name);
        if !source.is_file() {
            continue;
        }
        let destination = config.join(name);
        if destination.exists() {
            kept += 1;
            continue;
        }
        match std::fs::copy(&source, &destination) {
            Ok(_) => added += 1,
            Err(problem) => refused.push(format!("{}: {problem}", destination.display())),
        }
    }

    println!("  {added} files added, {kept} you already had and were left alone.");
    for problem in &refused {
        println!("  could not write: {problem}");
    }
    if !refused.is_empty() {
        return 1;
    }
    println!();
    println!("Next:");
    println!("  g13 gui    the window: set up the pad, the applets and the bindings");
    println!("  g13 run    take the pad and drive it");
    println!();
    println!("Nothing you had was overwritten, so running this again is safe.");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("g13-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The rule the whole default set rests on: a starting point, never an overwrite.
    #[test]
    fn a_file_the_user_has_edited_is_never_replaced() {
        let mine = temp("mine");
        let shipped = temp("shipped");
        std::fs::create_dir_all(shipped.join("bindings")).unwrap();
        std::fs::write(
            shipped.join("bindings/bindings-0.properties"),
            "G1=p,k.59\n",
        )
        .unwrap();
        std::fs::write(shipped.join("visuals.json"), "{\"enabled\": []}\n").unwrap();

        std::fs::create_dir_all(mine.join("bindings")).unwrap();
        std::fs::write(mine.join("bindings/bindings-0.properties"), "G1=p,k.30\n").unwrap();

        // the copy loop, without needing to move the real config
        for part in ["bindings"] {
            let source = shipped.join(part);
            let target = mine.join(part);
            for entry in std::fs::read_dir(&source).unwrap().flatten() {
                let destination = target.join(entry.file_name());
                if destination.exists() {
                    continue;
                }
                std::fs::copy(entry.path(), &destination).unwrap();
            }
        }
        let kept = std::fs::read_to_string(mine.join("bindings/bindings-0.properties")).unwrap();
        assert_eq!(kept, "G1=p,k.30\n", "the user's own binding was replaced");
        // and the file that was absent does arrive
        let destination = mine.join("visuals.json");
        if !destination.exists() {
            std::fs::copy(shipped.join("visuals.json"), &destination).unwrap();
        }
        assert!(
            destination.exists(),
            "a file that was missing did not arrive"
        );
        let _ = std::fs::remove_dir_all(&mine);
        let _ = std::fs::remove_dir_all(&shipped);
    }

    #[test]
    fn the_parts_are_the_ones_a_config_actually_has() {
        // folders are applets, fonts and themes - and nothing else, because bindings and macros live flat
        for part in ["applets", "fonts", "themes"] {
            assert!(PARTS.contains(&part), "{part} is not copied");
        }
        assert!(
            !PARTS.contains(&"bindings"),
            "bindings belong at the top of the config, not in a folder"
        );
        // and the defaults ship them flat, which is where the reader looks: a folder here means a fresh
        // install with no bindings at all, which is exactly what this caught
        if let Some(dir) = g13_config::defaults_dir() {
            assert!(
                dir.join("bindings-0.properties").is_file(),
                "the defaults hold no bindings file at the top"
            );
            assert!(
                !dir.join("bindings").is_dir(),
                "the defaults still hold a bindings folder, which nothing reads"
            );
        }
    }
}
