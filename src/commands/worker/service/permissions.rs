//! Restrict the service's data folder, then let its contents inherit that ACL.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

pub(super) fn lock_down(folder: &Path, account: &str) -> Result<()> {
    // Do not recurse here: /inheritance:r on each child removes the ACEs
    // just inherited from this folder and can leave worker.json unreadable.
    let locked = Command::new("icacls")
        .arg(folder)
        .args([
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-18:(OI)(CI)F",
            "*S-1-5-32-544:(OI)(CI)F",
            &format!("{account}:(OI)(CI)M"),
            "/Q",
        ])
        .status()
        .context("restricting the worker service data folder with icacls")?;
    anyhow::ensure!(
        locked.success(),
        "Could not set permissions on {} to allow only SYSTEM, Administrators and the PSF Guard Worker service account ({locked})",
        folder.display()
    );

    // Reset only the contents, never the protected root. This also repairs
    // children whose inheritance an earlier installation disabled, and gives
    // a replacement settings file the same permissions on reinstall.
    let inherited = Command::new("icacls")
        .arg(folder.join("*"))
        .args(["/reset", "/T", "/Q"])
        .status()
        .context("restoring worker service file inheritance with icacls")?;
    anyhow::ensure!(
        inherited.success(),
        "Could not restore file permissions in {} ({inherited}). Run worker-service install from PowerShell opened as administrator.",
        folder.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn powershell(script: &str, folder: &Path) -> std::process::Output {
        let result = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            // A parent PowerShell 7 may export module paths incompatible
            // with the Windows PowerShell used by this test.
            .env_remove("PSModulePath")
            .env("PSF_GUARD_ACL_TEST_FOLDER", folder)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        result
    }

    #[test]
    fn settings_and_task_files_inherit_permissions_on_install_and_reinstall() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Worker data with spaces");
        std::fs::create_dir_all(folder.join("tasks/nested")).unwrap();
        std::fs::write(folder.join("worker.json"), "test token").unwrap();
        std::fs::write(folder.join("tasks/nested/output.fits"), "test output").unwrap();
        let user = powershell(
            "[Security.Principal.WindowsIdentity]::GetCurrent().User.Value",
            &folder,
        );
        // Substitute the test user's SID for the service account. No service
        // installation or administrator privileges are needed in Windows CI.
        let account = format!("*{}", String::from_utf8(user.stdout).unwrap().trim());
        lock_down(&folder, &account).unwrap();
        powershell(include_str!("permissions.tests.ps1"), &folder);

        // Reproduce the old installer's empty protected child ACL, including
        // a nested task file, before checking that reinstall repairs it.
        powershell(
            r#"
$ErrorActionPreference = 'Stop'
$root = $env:PSF_GUARD_ACL_TEST_FOLDER
foreach ($name in @('worker.json', 'tasks/nested/output.fits')) {
    $path = Join-Path $root $name
    $acl = Get-Acl -LiteralPath $path
    $acl.SetAccessRuleProtection($true, $false)
    Set-Acl -LiteralPath $path -AclObject $acl
    if ((Get-Acl -LiteralPath $path).Access.Count -ne 0) { throw 'Expected an empty ACL' }
}
"#,
            &folder,
        );
        lock_down(&folder, &account).unwrap();
        powershell(include_str!("permissions.tests.ps1"), &folder);

        // Atomic settings replacement and newly created task files must
        // remain accessible without applying a separate file-specific ACL.
        let mut replacement = tempfile::NamedTempFile::new_in(&folder).unwrap();
        replacement.write_all(b"test token").unwrap();
        replacement.persist(folder.join("worker.json")).unwrap();
        std::fs::write(folder.join("tasks/new.fits"), "new output").unwrap();
        powershell(include_str!("permissions.tests.ps1"), &folder);
    }
}
