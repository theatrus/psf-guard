use std::{
    fs::{self, File},
    io,
    path::Path,
};
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Credential file protection is unavailable",
    )
}

#[cfg(unix)]
pub(super) fn open(path: &Path, create: bool) -> io::Result<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
    {
        return Err(denied());
    }
    Ok(file)
}
#[cfg(unix)]
pub(super) fn check_parent(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if !path.try_exists()? {
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
        return Err(denied());
    }
    Ok(())
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle},
        },
        ptr::null_mut,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SE_FILE_OBJECT,
            },
            *,
        },
        Storage::FileSystem::*,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    struct Local(*mut core::ffi::c_void);
    impl Drop for Local {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    fn user() -> io::Result<Vec<u64>> {
        unsafe {
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut size = 0;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            );
            CloseHandle(token);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(buffer)
        }
    }
    pub(crate) fn open(path: &Path, create: bool) -> io::Result<File> {
        unsafe {
            let user = user()?;
            let sid = (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid;
            let mut sid_string = null_mut();
            if ConvertSidToStringSidW(sid, &mut sid_string) == 0 {
                return Err(io::Error::last_os_error());
            }
            let _sid_string = Local(sid_string.cast());
            let mut len = 0;
            while *sid_string.add(len) != 0 {
                len += 1;
            }
            let sid_string = String::from_utf16_lossy(std::slice::from_raw_parts(sid_string, len));
            let sddl: Vec<u16> = format!("O:{sid_string}D:P(A;;FA;;;{sid_string})\0")
                .encode_utf16()
                .collect();
            let mut sd = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut sd,
                null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let _sd = Local(sd);
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            };
            let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &attributes,
                if create { CREATE_NEW } else { OPEN_EXISTING },
                FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let file = File::from_raw_handle(handle);
            if !file.metadata()?.is_file()
                || fs::symlink_metadata(path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(denied());
            }
            check_acl(&file, sid, false)?;
            Ok(file)
        }
    }
    unsafe fn check_acl(file: &File, user: PSID, parent: bool) -> io::Result<()> {
        unsafe {
            let mut owner = null_mut();
            let mut dacl = null_mut();
            let mut sd = null_mut();
            let result = GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut sd,
            );
            if result != 0 {
                return Err(io::Error::from_raw_os_error(result as i32));
            }
            let _sd = Local(sd);
            if owner.is_null()
                || dacl.is_null()
                || (EqualSid(owner, user) == 0
                    && (!parent
                        || (IsWellKnownSid(owner, WinLocalSystemSid) == 0
                            && IsWellKnownSid(owner, WinBuiltinAdministratorsSid) == 0)))
            {
                return Err(denied());
            }
            for i in 0..(*dacl).AceCount {
                let mut ace = null_mut();
                if GetAce(dacl, i as u32, &mut ace) == 0 {
                    return Err(denied());
                }
                let header = &*ace.cast::<ACE_HEADER>();
                if header.AceFlags as u32 & INHERIT_ONLY_ACE != 0 {
                    continue;
                }
                // Refuse unfamiliar ACE forms instead of overlooking a grant.
                if header.AceType != 0 {
                    if header.AceType == 1 {
                        continue;
                    }
                    return Err(denied());
                }
                let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                let sid = (&allowed.SidStart as *const u32).cast_mut().cast();
                let trusted = EqualSid(sid, user) != 0
                    || (parent
                        && (IsWellKnownSid(sid, WinLocalSystemSid) != 0
                            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0));
                if !trusted && (!parent || allowed.Mask & 0x500d0156 != 0) {
                    return Err(denied());
                }
            }
            Ok(())
        }
    }
    pub(crate) fn check_parent(path: &Path) -> io::Result<()> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        if !path.try_exists()? {
            fs::create_dir(path)?;
        }
        let m = fs::symlink_metadata(path)?;
        if !m.is_dir() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(denied());
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let user = user()?;
        unsafe {
            check_acl(
                &file,
                (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid,
                true,
            )
        }
    }
    #[cfg(test)]
    pub(crate) fn private_test_directory(path: &Path) -> io::Result<()> {
        // The desktop test harness grants extra users access to TEMP. Fixtures
        // need a real private directory; do not weaken the production check.
        unsafe {
            let user = user()?;
            let sid = (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid;
            let mut text = null_mut();
            if ConvertSidToStringSidW(sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let _text = Local(text.cast());
            let mut len = 0;
            while *text.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})\0")
                .encode_utf16()
                .collect();
            let mut sd = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut sd,
                null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let _sd = Local(sd);
            let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if SetFileSecurityW(
                name.as_ptr(),
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                sd,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
    use std::os::windows::fs::MetadataExt;
}
#[cfg(all(windows, test))]
pub(crate) use windows::private_test_directory;
#[cfg(windows)]
pub(super) use windows::{check_parent, open};
#[cfg(all(unix, test))]
pub(crate) fn private_test_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
