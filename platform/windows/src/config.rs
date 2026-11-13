//! Where the portable build keeps its settings.
//!
//! Beside the executable, because that is what portable means: unzip the
//! folder onto a stick, and the colours and the queue travel with it. The
//! profile directory is only a fallback, for the case where the folder
//! cannot be written to -- unzipped into Program Files, say, or opened from
//! a read-only share -- where losing every setting silently would be worse
//! than keeping them somewhere less portable.
//!
//! The *format* is `whitescreen_core::settings`, the same one the GTK build
//! reads and writes, so a settings file copied between the two works.

use std::path::{Path, PathBuf};

/// The file name used beside the executable.
const PORTABLE_NAME: &str = "whitescreen.ini";

/// Where to read and write settings.
pub fn settings_path() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let profile = std::env::var_os("APPDATA").map(PathBuf::from);
    choose(beside.as_deref().filter(|dir| is_writable(dir)), profile)
}

/// Pick between the two, given which of them is usable.
///
/// Split out from the probing so the rule can be tested: the folder beside
/// the executable wins whenever it can be written to, and the profile is
/// what is left when it cannot.
fn choose(writable_beside: Option<&Path>, profile: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = writable_beside {
        return dir.join(PORTABLE_NAME);
    }
    match profile {
        Some(dir) => dir.join("whitescreen").join("settings.ini"),
        // Nowhere else to go; the relative path at least keeps the app
        // working in whatever directory it was started from.
        None => PathBuf::from(PORTABLE_NAME),
    }
}

/// Whether a directory can be written to, by trying rather than by asking.
///
/// Windows permissions are not a question that can be answered honestly
/// from the attributes alone -- an ACL, a read-only share or virtualisation
/// can all override them -- so the only reliable check is to write.
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".whitescreen-write-test");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_writable_folder_beside_the_executable_wins() {
        let beside = PathBuf::from("/apps/whitescreen");
        let chosen = choose(Some(&beside), Some(PathBuf::from("/users/me/AppData")));
        assert_eq!(chosen, beside.join(PORTABLE_NAME), "portable comes first");
    }

    #[test]
    fn a_read_only_folder_falls_back_to_the_profile() {
        let chosen = choose(None, Some(PathBuf::from("/users/me/AppData")));
        assert_eq!(
            chosen,
            PathBuf::from("/users/me/AppData").join("whitescreen").join("settings.ini")
        );
    }

    #[test]
    fn with_nowhere_to_write_it_still_names_a_file() {
        // Losing the settings is acceptable; panicking is not.
        assert_eq!(choose(None, None), PathBuf::from(PORTABLE_NAME));
    }

    #[test]
    fn a_real_directory_is_detected_as_writable() {
        let dir = std::env::temp_dir();
        assert!(is_writable(&dir), "the temp directory should be writable");
        assert!(!is_writable(Path::new("/nonexistent-whitescreen-dir")));
    }
}
