use super::*;
use std::os::unix::fs::PermissionsExt as _;

fn scratch_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "sway-launch-setup-file-test-{}-{}",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_dir_all(&directory);
    directory
}

// setup_file_directory

#[test]
fn setup_files_go_under_the_runtime_directory() {
    assert_eq!(
        setup_file_directory(Some(OsStr::new("/run/user/1000"))),
        Some(PathBuf::from("/run/user/1000/sway-launch"))
    );
}

#[test]
fn no_runtime_directory_means_no_setup_file() {
    assert_eq!(setup_file_directory(None), None);
    assert_eq!(setup_file_directory(Some(OsStr::new(""))), None);
}

// SetupFile::write_in

#[test]
fn a_setup_file_holds_the_setup_and_only_its_owner_can_read_it() {
    let directory = scratch_directory("contents");
    let file = SetupFile::write_in(&directory, "export TOKEN='secret'", "1-2-3").unwrap();
    assert_eq!(file.path(), directory.join("launch-1-2-3"));
    assert_eq!(
        std::fs::read_to_string(file.path()).unwrap(),
        "export TOKEN='secret'\n"
    );
    let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let directory_mode = std::fs::metadata(&directory).unwrap().permissions().mode();
    assert_eq!(directory_mode & 0o777, 0o700);
    drop(file);
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn a_setup_file_is_removed_when_dropped() {
    let directory = scratch_directory("drop");
    let file = SetupFile::write_in(&directory, "cd '/'", "4-5-6").unwrap();
    let path = file.path().to_path_buf();
    assert!(path.exists());
    drop(file);
    assert!(!path.exists());
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn a_setup_file_never_overwrites_an_existing_file() {
    let directory = scratch_directory("existing");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("launch-7-8-9"), "planted").unwrap();
    let error = SetupFile::write_in(&directory, "cd '/'", "7-8-9").unwrap_err();
    assert!(error.contains("launch-7-8-9"), "{error}");
    assert_eq!(
        std::fs::read_to_string(directory.join("launch-7-8-9")).unwrap(),
        "planted"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}
