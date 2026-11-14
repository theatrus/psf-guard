//! The Windows account the service runs as: by default the person who
//! installs it, so the worker has their RC-Astro license and their access
//! to network shares. A user account needs its password and the "Log on as
//! a service" right; the old virtual account needs neither but leaves a
//! profile behind.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use anyhow::{Context, Result};
use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_ACCOUNT_DISABLED, ERROR_ACCOUNT_RESTRICTION,
    ERROR_FILE_NOT_FOUND, ERROR_LOGON_FAILURE, ERROR_LOGON_TYPE_NOT_GRANTED, ERROR_NOT_FOUND,
    ERROR_PASSWORD_EXPIRED, ERROR_PATH_NOT_FOUND, HANDLE, NTSTATUS,
};
use windows_sys::Win32::Security::Authentication::Identity::{
    GetUserNameExW, LsaAddAccountRights, LsaClose, LsaEnumerateAccountRights, LsaFreeMemory,
    LsaNtStatusToWinError, LsaOpenPolicy, LsaRemoveAccountRights, NameSamCompatible, LSA_HANDLE,
    LSA_OBJECT_ATTRIBUTES, LSA_UNICODE_STRING, POLICY_CREATE_ACCOUNT, POLICY_LOOKUP_NAMES,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows_sys::Win32::Security::{
    LogonUserW, LookupAccountNameW, LookupAccountSidW, SidTypeUser, LOGON32_LOGON_SERVICE,
    LOGON32_PROVIDER_DEFAULT, PSID, SID_NAME_USE,
};
use windows_sys::Win32::UI::Shell::DeleteProfileW;

/// The right a user account needs to run a service.
const SERVICE_LOGON_RIGHT: &str = "SeServiceLogonRight";
/// What LsaEnumerateAccountRights returns for an account with no rights of
/// its own.
const STATUS_OBJECT_NAME_NOT_FOUND: NTSTATUS = 0xC000_0034_u32 as i32;

/// A Windows account: `DOMAIN\name` as Windows spells it, and its SID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Account {
    pub name: String,
    pub sid: String,
    /// A person's account, rather than a group or a service's own account.
    pub user: bool,
}

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// The account running this command.
pub(super) fn current() -> Result<Account> {
    let mut size = 0u32;
    // The first call reports the size, terminator included.
    unsafe { GetUserNameExW(NameSamCompatible, ptr::null_mut(), &mut size) };
    let mut buffer = vec![0u16; size.max(1) as usize];
    if !unsafe { GetUserNameExW(NameSamCompatible, buffer.as_mut_ptr(), &mut size) } {
        return Err(last_error()).context("finding the account running this command");
    }
    buffer.truncate(size as usize);
    lookup(&String::from_utf16_lossy(&buffer))
}

/// Find an account by name, such as `ann`, `PC\ann` or `DOMAIN\ann`.
pub(super) fn lookup(name: &str) -> Result<Account> {
    // `.\ann`, the Services console's spelling, means this computer.
    let name = match name.strip_prefix(r".\") {
        Some(user) => match std::env::var("COMPUTERNAME") {
            Ok(computer) => format!(r"{computer}\{user}"),
            Err(_) => user.to_string(),
        },
        None => name.to_string(),
    };
    let wname = wide(&name);
    let (mut sid_size, mut domain_size, mut kind) = (0u32, 0u32, 0 as SID_NAME_USE);
    unsafe {
        LookupAccountNameW(
            ptr::null(),
            wname.as_ptr(),
            ptr::null_mut(),
            &mut sid_size,
            ptr::null_mut(),
            &mut domain_size,
            &mut kind,
        )
    };
    if sid_size == 0 {
        return Err(last_error()).with_context(|| format!("No Windows account {name} was found"));
    }
    let mut sid = vec![0u8; sid_size as usize];
    let mut domain = vec![0u16; domain_size.max(1) as usize];
    let found = unsafe {
        LookupAccountNameW(
            ptr::null(),
            wname.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        )
    };
    if found == 0 {
        return Err(last_error()).with_context(|| format!("No Windows account {name} was found"));
    }
    let psid: PSID = sid.as_mut_ptr().cast();
    Ok(Account {
        name: canonical_name(psid).unwrap_or(name),
        sid: sid_string(psid)?,
        user: kind == SidTypeUser,
    })
}

/// `DOMAIN\name` as Windows spells it, which the Services console shows.
fn canonical_name(sid: PSID) -> Option<String> {
    let (mut name_size, mut domain_size, mut kind) = (0u32, 0u32, 0 as SID_NAME_USE);
    unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid,
            ptr::null_mut(),
            &mut name_size,
            ptr::null_mut(),
            &mut domain_size,
            &mut kind,
        )
    };
    if name_size == 0 {
        return None;
    }
    let mut name = vec![0u16; name_size as usize];
    let mut domain = vec![0u16; domain_size.max(1) as usize];
    let found = unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        )
    };
    if found == 0 {
        return None;
    }
    let name = String::from_utf16_lossy(&name[..name_size as usize]);
    let domain = String::from_utf16_lossy(&domain[..domain_size as usize]);
    Some(if domain.is_empty() {
        name
    } else {
        format!(r"{domain}\{name}")
    })
}

fn sid_string(sid: PSID) -> Result<String> {
    let mut text: PWSTR = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(last_error()).context("reading an account's SID");
    }
    let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok(value)
}

/// A SID from its string form, freed on drop.
struct Sid(PSID);

impl Sid {
    fn parse(sid: &str) -> Result<Self> {
        let text = wide(sid);
        let mut parsed: PSID = ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut parsed) } == 0 {
            return Err(last_error()).with_context(|| format!("reading the SID {sid}"));
        }
        Ok(Self(parsed))
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0.cast()) };
    }
}

fn nt(status: NTSTATUS) -> std::io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(
            unsafe { LsaNtStatusToWinError(status) } as i32,
        ))
    }
}

/// This computer's security policy, closed on drop.
struct Policy(LSA_HANDLE);

impl Policy {
    fn open() -> Result<Self> {
        let attributes = LSA_OBJECT_ATTRIBUTES::default();
        let mut handle: LSA_HANDLE = 0;
        nt(unsafe {
            LsaOpenPolicy(
                ptr::null(),
                &attributes,
                (POLICY_CREATE_ACCOUNT | POLICY_LOOKUP_NAMES) as u32,
                &mut handle,
            )
        })
        .context("opening this computer's security policy")?;
        Ok(Self(handle))
    }
}

impl Drop for Policy {
    fn drop(&mut self) {
        unsafe { LsaClose(self.0) };
    }
}

/// `text` as an LSA string over `buffer`, which must outlive it.
fn lsa_string(buffer: &mut [u16]) -> LSA_UNICODE_STRING {
    // Lengths are in bytes and leave out the terminator.
    let bytes = ((buffer.len() - 1) * 2) as u16;
    LSA_UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes + 2,
        Buffer: buffer.as_mut_ptr(),
    }
}

/// Whether the account holds "Log on as a service" itself, not through a
/// group.
fn has_service_logon(policy: &Policy, sid: &Sid) -> Result<bool> {
    let mut rights: *mut LSA_UNICODE_STRING = ptr::null_mut();
    let mut count = 0u32;
    let status = unsafe { LsaEnumerateAccountRights(policy.0, sid.0, &mut rights, &mut count) };
    if status == STATUS_OBJECT_NAME_NOT_FOUND {
        return Ok(false);
    }
    nt(status).context("reading the account's rights")?;
    let wanted: Vec<u16> = SERVICE_LOGON_RIGHT.encode_utf16().collect();
    let held = unsafe { std::slice::from_raw_parts(rights, count as usize) }
        .iter()
        .any(|right| {
            let text =
                unsafe { std::slice::from_raw_parts(right.Buffer, right.Length as usize / 2) };
            text == wanted.as_slice()
        });
    unsafe { LsaFreeMemory(rights.cast_const().cast()) };
    Ok(held)
}

/// Give the account "Log on as a service". Returns whether this added it,
/// so that uninstalling takes away only what installing gave.
pub(super) fn grant_service_logon(account: &Account) -> Result<bool> {
    let policy = Policy::open()?;
    let sid = Sid::parse(&account.sid)?;
    if has_service_logon(&policy, &sid)? {
        return Ok(false);
    }
    let mut buffer = wide(SERVICE_LOGON_RIGHT);
    let right = lsa_string(&mut buffer);
    nt(unsafe { LsaAddAccountRights(policy.0, sid.0, &right, 1) })
        .with_context(|| format!("giving {} the right to log on as a service", account.name))?;
    Ok(true)
}

/// Take away "Log on as a service", which installing gave.
pub(super) fn revoke_service_logon(sid: &str) -> Result<()> {
    let policy = Policy::open()?;
    let sid = Sid::parse(sid)?;
    let mut buffer = wide(SERVICE_LOGON_RIGHT);
    let right = lsa_string(&mut buffer);
    nt(unsafe { LsaRemoveAccountRights(policy.0, sid.0, false, &right, 1) })
        .context("removing the right to log on as a service")?;
    Ok(())
}

/// Log on as the service will, to check the password before the service
/// depends on it. The account must already hold "Log on as a service".
pub(super) fn check_password(account: &Account, password: &str) -> Result<()> {
    let (domain, user) = account
        .name
        .split_once('\\')
        .unwrap_or((".", account.name.as_str()));
    let (user, domain) = (wide(user), wide(domain));
    let mut secret = wide(password);
    let mut token: HANDLE = ptr::null_mut();
    let logged_on = unsafe {
        LogonUserW(
            user.as_ptr(),
            domain.as_ptr(),
            secret.as_ptr(),
            LOGON32_LOGON_SERVICE,
            LOGON32_PROVIDER_DEFAULT,
            &mut token,
        )
    };
    let error = unsafe { GetLastError() };
    secret.fill(0);
    if logged_on != 0 {
        unsafe { CloseHandle(token) };
        return Ok(());
    }
    let name = &account.name;
    anyhow::bail!(match error {
        ERROR_LOGON_FAILURE => format!(
            "Windows did not accept the password for {name}. For a Microsoft account, \
             enter the account's password, not its PIN."
        ),
        ERROR_ACCOUNT_RESTRICTION => format!(
            "{name} has no password, and Windows does not let a service log on without one. \
             Set a password for the account, or install with --virtual-account."
        ),
        ERROR_LOGON_TYPE_NOT_GRANTED => format!(
            "A policy on this computer stops {name} from logging on as a service. Ask the \
             computer's administrator, or install with --virtual-account."
        ),
        ERROR_PASSWORD_EXPIRED =>
            format!("The password for {name} has expired. Change it, then install again."),
        ERROR_ACCOUNT_DISABLED => format!("The account {name} is disabled."),
        _ => format!(
            "Windows could not log on as {name}: {}",
            std::io::Error::from_raw_os_error(error as i32)
        ),
    })
}

/// Delete an account's profile folder and its registry entry. Returns
/// whether there was one.
pub(super) fn delete_profile(sid: &str) -> Result<bool> {
    let text = wide(sid);
    if unsafe { DeleteProfileW(text.as_ptr(), ptr::null(), ptr::null()) } != 0 {
        return Ok(true);
    }
    match unsafe { GetLastError() } {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_NOT_FOUND => Ok(false),
        error => Err(std::io::Error::from_raw_os_error(error as i32))
            .with_context(|| format!("deleting the profile of {sid}")),
    }
}
