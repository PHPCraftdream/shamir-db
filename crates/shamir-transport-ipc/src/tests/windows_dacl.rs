use std::ffi::c_void;
use std::io;
use std::mem::MaybeUninit;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorDacl,
    GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION,
    DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(crate) fn owner_only_dacl(descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: descriptor is a valid security descriptor; output pointers refer to writable locals.
    let ok =
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) };
    if ok == 0 || present == 0 || dacl.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "DACL absent",
        ));
    }

    let mut size = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: dacl is owned by descriptor; size is a writable output buffer of the documented size.
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut size as *mut ACL_SIZE_INFORMATION).cast::<c_void>(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || size.AceCount != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "DACL ACE count",
        ));
    }

    let mut raw_ace: *mut c_void = ptr::null_mut();
    // SAFETY: index zero is in range after confirming exactly one ACE.
    if unsafe { GetAce(dacl, 0, &mut raw_ace) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
    // SAFETY: GetAce returned a pointer into the validated ACL; check ACE type before accessing fields.
    if unsafe { (*ace).Header.AceType } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "ACE is not allow",
        ));
    }
    // SAFETY: for an ACCESS_ALLOWED_ACE, SidStart begins the SID stored in this ACL ACE.
    let sid = unsafe { (&mut (*ace).SidStart) as *mut u32 as PSID };

    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: current-process pseudo-handle and writable token output are valid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut needed = 0;
    // SAFETY: documented null-buffer query obtains the required TokenUser buffer size.
    unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        let error = io::Error::last_os_error();
        // SAFETY: token was returned by OpenProcessToken and is closed exactly once.
        unsafe { CloseHandle(token) };
        return Err(error);
    }
    let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
    let mut bytes = vec![MaybeUninit::<usize>::uninit(); words];
    // SAFETY: MaybeUninit<usize> provides suitable alignment and the allocation is at least `needed` bytes.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            bytes.as_mut_ptr().cast::<c_void>(),
            needed,
            &mut needed,
        )
    };
    let error = (ok == 0).then(io::Error::last_os_error);
    // SAFETY: token was returned by OpenProcessToken and is closed exactly once.
    unsafe { CloseHandle(token) };
    if let Some(error) = error {
        return Err(error);
    }

    let user = bytes.as_ptr().cast::<TOKEN_USER>();
    // SAFETY: successful GetTokenInformation initialized TOKEN_USER; EqualSid reads both valid SIDs.
    if unsafe { EqualSid(sid, (*user).User.Sid) } == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "allow SID differs from current user",
        ));
    }
    Ok(())
}

pub(crate) fn get_security_info(handle: HANDLE) -> io::Result<PSECURITY_DESCRIPTOR> {
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: handle is a live borrowed kernel-object handle; outputs are valid and descriptor ownership transfers to caller.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(descriptor)
}

pub(crate) fn free_descriptor(descriptor: PSECURITY_DESCRIPTOR) {
    // SAFETY: descriptor was allocated by GetSecurityInfo or ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(descriptor) };
}

pub(crate) fn check_handle_owner_dacl(handle: HANDLE) -> io::Result<()> {
    let descriptor = get_security_info(handle)?;
    let result = owner_only_dacl(descriptor);
    free_descriptor(descriptor);
    result
}

#[tokio::test]
async fn negative_sddl_controls_fail_owner_only_check() {
    for sddl in ["D:(A;;GA;;;WD)", "D:NO_ACCESS_CONTROL"] {
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: wide is NUL-terminated UTF-16 and descriptor points to writable output; success allocation is freed below.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0, "valid test SDDL");
        let result = owner_only_dacl(descriptor);
        free_descriptor(descriptor);
        assert!(
            result.is_err(),
            "negative-control SDDL unexpectedly passed: {sddl}"
        );
    }
}
