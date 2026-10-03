//! Win32 handles and descriptors remain inside this security boundary.
use std::{
    ffi::c_void,
    fs::File,
    io::{self, Read, Write},
    os::windows::{
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::Path,
    ptr,
};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SE_FILE_OBJECT,
        },
        DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation, OWNER_SECURITY_INFORMATION,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    System::{
        Pipes::GetNamedPipeClientProcessId,
        Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns exactly one successfully opened handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: Win32 allocated this buffer with LocalAlloc, and it is freed once.
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

// SAFETY: callers supply a SID owned by a live OS token/security descriptor.
unsafe fn sid_string(sid: *mut c_void) -> io::Result<String> {
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(text.cast());
    let mut length = 0;
    // SAFETY: ConvertSidToStringSidW returns an allocated, NUL-terminated string.
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, length)).map_err(io::Error::other)
    }
}

fn process_sid(process: HANDLE) -> io::Result<String> {
    let mut token = ptr::null_mut();
    // SAFETY: process is a valid borrowed handle; token is an output pointer.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut length = 0;
    // SAFETY: a null buffer with length zero queries the required allocation.
    unsafe {
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut length);
    }
    if length < std::mem::size_of::<TOKEN_USER>() as u32 || length > 65_536 {
        return Err(io::Error::other("invalid token information size"));
    }
    // usize storage ensures TOKEN_USER alignment, including on 64-bit Windows.
    let mut storage = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    // SAFETY: storage is aligned, writable and at least length bytes long.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful call initialized TOKEN_USER and its embedded SID.
    unsafe { sid_string((*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid) }
}

pub fn current_user_sid() -> io::Result<String> {
    // SAFETY: the pseudo handle is borrowed and must not be closed.
    process_sid(unsafe { GetCurrentProcess() })
}

/// Read-only lifetime check for the daemon's update supervisor.
pub fn process_alive(pid: u32) -> io::Result<bool> {
    use windows_sys::Win32::System::Threading::GetExitCodeProcess;
    // SAFETY: query-only access; the owned handle is closed by Handle.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Ok(false);
    }
    let process = Handle(process);
    let mut status = 0;
    // SAFETY: process is valid and status is a writable output pointer.
    if unsafe { GetExitCodeProcess(process.0, &mut status) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(status == 259) // STILL_ACTIVE
}

pub fn verify_pipe_peer(pipe: &NamedPipeServer) -> io::Result<()> {
    let mut pid = 0;
    // SAFETY: Tokio owns a connected server pipe for the duration of this call.
    if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: requesting read-only process-token access; failure is not bypassed.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(process);
    if process_sid(process.0)? != current_user_sid()? {
        return Err(denied("pipe client belongs to another Windows user"));
    }
    Ok(())
}

pub fn create_user_pipe(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    if !name.starts_with(r"\\.\pipe\e2em-")
        || name.len() > 200
        || !name[r"\\.\pipe\e2em-".len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || name.len() == r"\\.\pipe\e2em-".len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid local E2EM pipe name",
        ));
    }
    let sid = current_user_sid()?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})(A;;GA;;;SY)")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: sddl is NUL terminated, and descriptor is an output pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: attributes and its descriptor live through pipe creation. Windows
    // copies the descriptor; Tokio owns the returned overlapped handle.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                (&attributes as *const SECURITY_ATTRIBUTES)
                    .cast_mut()
                    .cast(),
            )
    }
}

/// Read an opened file only after checking its owner and every granting ACE.
/// Same-user processes remain part of the trust boundary, as on Unix.
pub fn read_private_file(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 || metadata.len() > 65_536 {
        return Err(denied("grants must be a bounded non-reparse regular file"));
    }
    let mut file = File::open(path)?;
    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: all output pointers are valid; the file handle remains open.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _allocation = LocalAllocation(descriptor);
    let sid = current_user_sid()?;
    // SAFETY: owner and DACL are inside the live security descriptor.
    unsafe {
        if owner.is_null() || sid_string(owner)? != sid || dacl.is_null() {
            return Err(denied("grants owner or DACL is not private"));
        }
        for index in 0..(*dacl).AceCount {
            let mut ace = ptr::null_mut();
            if GetAce(dacl, u32::from(index), &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let header = &*ace.cast::<ACE_HEADER>();
            match header.AceType {
                0 => {
                    // ACCESS_ALLOWED_ACE
                    let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                    let grantee = sid_string((&allowed.SidStart as *const u32).cast_mut().cast())?;
                    if grantee != sid && grantee != "S-1-5-18" {
                        return Err(denied("grants ACL allows another principal"));
                    }
                }
                1 => (), // ACCESS_DENIED_ACE grants no access.
                _ => return Err(denied("unsupported grants ACL entry")),
            }
        }
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(65_537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err(denied("grant file grew beyond its limit"));
    }
    Ok(bytes)
}

/// Create a new private grant/credential file; never replace an existing path.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL};
    let sid = current_user_sid()?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})(A;;GA;;;SY)")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: the NUL-terminated input and output descriptor pointer are valid.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if name[..name.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in private file path",
        ));
    }
    // SAFETY: CreateFileW copies the descriptor; CREATE_NEW rejects existing paths.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the successfully created handle passes exactly once.
    let mut file = unsafe { File::from_raw_handle(handle) };
    file.write_all(bytes)?;
    file.sync_all()
}
