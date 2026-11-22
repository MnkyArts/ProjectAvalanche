use std::time::SystemTime;

/// Returns true if nth bit in array is 1.
#[allow(dead_code)]
pub(crate) fn test_bit(n: u16, array: &[u8]) -> bool {
    (array[(n / 8) as usize] >> (n % 8)) & 1 != 0
}

#[cfg(not(target_arch = "wasm32"))]
pub fn time_now() -> SystemTime {
    SystemTime::now()
}

#[cfg(target_arch = "wasm32")]
pub fn time_now() -> SystemTime {
    use js_sys::Date;
    use std::time::Duration;

    let offset = Duration::from_millis(Date::now() as u64);
    SystemTime::UNIX_EPOCH + offset
}

/// Project Avalanche patch T26: whether the process runs under Wine (Proton among it), as SDL 3.4.20 asks
/// (`WIN_IsWine`, src/core/windows/SDL_windows.c:335-351): ntdll.dll exports `wine_get_version` under Wine only.
/// Asked once; the answer is kept, as SDL keeps it.
#[cfg(target_os = "windows")]
pub fn is_wine() -> bool {
    use std::ffi::{c_char, c_void};
    use std::sync::OnceLock;

    // kernel32's own declarations (LoadLibraryW, GetProcAddress, FreeLibrary): HMODULE and FARPROC are pointers.
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }

    static IS_WINE: OnceLock<bool> = OnceLock::new();
    *IS_WINE.get_or_init(|| {
        let ntdll_name: Vec<u16> = "ntdll.dll".encode_utf16().chain([0]).collect();
        // SAFETY: LoadLibraryW gets a NUL-terminated UTF-16 name; GetProcAddress the module it returned and a
        // NUL-terminated name; FreeLibrary releases that module once, after the last use.
        unsafe {
            let ntdll = LoadLibraryW(ntdll_name.as_ptr());
            if ntdll.is_null() {
                return false;
            }
            let found = !GetProcAddress(ntdll, c"wine_get_version".as_ptr()).is_null();
            FreeLibrary(ntdll);
            found
        }
    })
}
