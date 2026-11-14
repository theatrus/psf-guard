//! Restrict the service's data folder, then let its contents inherit that ACL.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::ptr;

use anyhow::{Context, Result};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SET_ACCESS,
    SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};

use super::account::Sid;

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// What `icacls` calls Modify: everything but changing permissions and
/// taking ownership.
const MODIFY: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

/// Give `folder` exactly these entries: SYSTEM and Administrators with full
/// control, and the service's account, by SID, with Modify. The folder's
/// whole access list is replaced in one call, so no earlier account, entry
/// added by hand or entry inherited from ProgramData stays. `account` is
/// `None` until a new virtual account exists.
pub(super) fn lock_down(folder: &Path, account: Option<&str>) -> Result<()> {
    let mut entries = vec![(SYSTEM, FILE_ALL_ACCESS), (ADMINISTRATORS, FILE_ALL_ACCESS)];
    if let Some(account) = account {
        entries.push((account, MODIFY));
    }
    let sids = entries
        .iter()
        .map(|(sid, _)| Sid::parse(sid))
        .collect::<Result<Vec<_>>>()?;
    let access: Vec<EXPLICIT_ACCESS_W> = entries
        .iter()
        .zip(&sids)
        .map(|((_, rights), sid)| EXPLICIT_ACCESS_W {
            grfAccessPermissions: *rights,
            grfAccessMode: SET_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: sid.as_ptr().cast(),
            },
        })
        .collect();
    let mut acl: *mut ACL = ptr::null_mut();
    let built =
        unsafe { SetEntriesInAclW(access.len() as u32, access.as_ptr(), ptr::null(), &mut acl) };
    if built != 0 {
        return Err(std::io::Error::from_raw_os_error(built as i32))
            .context("building the worker service data folder's permissions");
    }
    let name: Vec<u16> = folder
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Protected: nothing is inherited from ProgramData. Windows passes the
    // new entries down to every child that inherits.
    let set = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    unsafe { LocalFree(acl.cast()) };
    drop(sids);
    if set != 0 {
        return Err(std::io::Error::from_raw_os_error(set as i32)).with_context(|| {
            format!(
                "Could not set permissions on {} to allow only SYSTEM, Administrators and the \
                 PSF Guard Worker service account",
                folder.display()
            )
        });
    }

    // Reset only the contents, never the protected root. This also repairs
    // children whose inheritance an earlier installation disabled, and gives
    // a replacement settings file the same permissions on reinstall.
    if std::fs::read_dir(folder)?.next().is_none() {
        return Ok(());
    }
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
        let account = String::from_utf8(user.stdout).unwrap().trim().to_string();
        lock_down(&folder, Some(&account)).unwrap();
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
        lock_down(&folder, Some(&account)).unwrap();
        powershell(include_str!("permissions.tests.ps1"), &folder);

        // Atomic settings replacement and newly created task files must
        // remain accessible without applying a separate file-specific ACL.
        let mut replacement = tempfile::NamedTempFile::new_in(&folder).unwrap();
        replacement.write_all(b"test token").unwrap();
        replacement.persist(folder.join("worker.json")).unwrap();
        std::fs::write(folder.join("tasks/new.fits"), "new output").unwrap();
        powershell(include_str!("permissions.tests.ps1"), &folder);

        // Moving the service to another account leaves no trace of the old
        // one on the folder or any file in it, nor of an entry added by
        // hand. BUILTIN\Users stands in for the old virtual account.
        lock_down(&folder, Some("S-1-5-32-545")).unwrap();
        powershell(
            r#"
$ErrorActionPreference = 'Stop'
$root = $env:PSF_GUARD_ACL_TEST_FOLDER
$acl = Get-Acl -LiteralPath $root
$everyone = New-Object Security.Principal.SecurityIdentifier('S-1-1-0')
$rule = New-Object Security.AccessControl.FileSystemAccessRule($everyone, 'Read', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $root -AclObject $acl
"#,
            &folder,
        );
        lock_down(&folder, Some(&account)).unwrap();
        powershell(include_str!("permissions.tests.ps1"), &folder);
    }
}
