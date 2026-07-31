//! End-to-end coverage for server users.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_psf-guard"))
        .args(args)
        .output()
        .expect("running psf-guard")
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn add_list_replace_and_remove_users() {
    let directory = tempfile::tempdir().unwrap();
    let database_registry = directory.path().join("test-config.json");
    let password_file = directory.path().join("password");
    fs::write(&password_file, "long-user-password\n").unwrap();
    let registry_arg = path_arg(&database_registry);
    let password_arg = path_arg(&password_file);

    let added = run(&[
        "users",
        "add",
        "viewer",
        "--role",
        "read-only",
        "--email",
        "viewer@example.com",
        "--password-file",
        &password_arg,
        "--registry",
        &registry_arg,
    ]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );

    let auth_path = directory.path().join("test-config.auth.json");
    let contents = fs::read_to_string(&auth_path).unwrap();
    assert!(contents.contains("\"username\": \"viewer\""));
    assert!(contents.contains("\"role\": \"read_only\""));
    assert!(contents.contains("\"email\": \"viewer@example.com\""));
    assert!(!contents.contains("long-user-password"));
    assert!(contents.contains("$argon2"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&auth_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let listed = run(&["users", "list", "--registry", &registry_arg]);
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.status.success(), "{stdout}");
    assert!(stdout.contains("viewer"));
    assert!(stdout.contains("read-only"));
    assert!(stdout.contains("viewer@example.com"));

    let duplicate = run(&[
        "users",
        "add",
        "viewer",
        "--role",
        "read-write",
        "--password-file",
        &password_arg,
        "--registry",
        &registry_arg,
    ]);
    assert!(!duplicate.status.success());
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("--replace"),
        "{}",
        String::from_utf8_lossy(&duplicate.stderr)
    );

    let replaced = run(&[
        "users",
        "add",
        "viewer",
        "--role",
        "read-write",
        "--password-file",
        &password_arg,
        "--replace",
        "--registry",
        &registry_arg,
    ]);
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert!(fs::read_to_string(&auth_path)
        .unwrap()
        .contains("\"email\": \"viewer@example.com\""));

    let guarded_remove = run(&["users", "remove", "viewer", "--registry", &registry_arg]);
    assert!(!guarded_remove.status.success());
    assert!(
        String::from_utf8_lossy(&guarded_remove.stderr).contains("--allow-empty"),
        "{}",
        String::from_utf8_lossy(&guarded_remove.stderr)
    );

    let removed = run(&[
        "users",
        "remove",
        "viewer",
        "--allow-empty",
        "--registry",
        &registry_arg,
    ]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let contents = fs::read_to_string(auth_path).unwrap();
    assert!(contents.contains("\"users\": []"));
}
