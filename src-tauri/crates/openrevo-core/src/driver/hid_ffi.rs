// OpenRevo - Unified Windows Win32 HID, SetupAPI, Kernel32 & User32 FFI Layer
// Extracted to centralize low-level hardware communication and eliminate duplication.

#![allow(dead_code, non_snake_case, non_camel_case_types)]

use std::ffi::c_void;

pub type HANDLE = *mut c_void;
pub type BOOLEAN = u8;

pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
pub const DIGCF_PRESENT: u32 = 0x00000002;
pub const DIGCF_DEVICEINTERFACE: u32 = 0x00000010;
pub const GENERIC_READ: u32 = 0x80000000;
pub const GENERIC_WRITE: u32 = 0x40000000;
pub const FILE_SHARE_READ: u32 = 0x00000001;
pub const FILE_SHARE_WRITE: u32 = 0x00000002;
pub const OPEN_EXISTING: u32 = 3;
pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
pub const FILE_FLAG_OVERLAPPED: u32 = 0x40000000;
pub const ERROR_IO_PENDING: u32 = 997;
pub const WAIT_OBJECT_0: u32 = 0;

#[repr(C)]
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct HIDD_ATTRIBUTES {
    pub size: u32,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_number: u16,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct HIDP_CAPS {
    pub usage: u16,
    pub usage_page: u16,
    pub input_report_byte_length: u16,
    pub output_report_byte_length: u16,
    pub feature_report_byte_length: u16,
    pub reserved: [u16; 17],
    pub number_link_collection_nodes: u16,
    pub number_input_button_caps: u16,
    pub number_input_value_caps: u16,
    pub number_input_data_indices: u16,
    pub number_output_button_caps: u16,
    pub number_output_value_caps: u16,
    pub number_output_data_indices: u16,
    pub number_feature_button_caps: u16,
    pub number_feature_value_caps: u16,
    pub number_feature_data_indices: u16,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct SP_DEVICE_INTERFACE_DATA {
    pub cb_size: u32,
    pub interface_class_guid: GUID,
    pub flags: u32,
    pub reserved: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct LARGE_INTEGER {
    pub quad_part: i64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct OVERLAPPED {
    pub internal: usize,
    pub internal_high: usize,
    pub offset: u32,
    pub offset_high: u32,
    pub h_event: HANDLE,
}

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct LASTINPUTINFO {
    pub cb_size: u32,
    pub dw_time: u32,
}

#[link(name = "hid")]
extern "system" {
    pub fn HidD_GetHidGuid(hid_guid: *mut GUID);
    pub fn HidD_GetAttributes(hid_device_object: HANDLE, attributes: *mut HIDD_ATTRIBUTES) -> BOOLEAN;
    pub fn HidD_GetPreparsedData(hid_device_object: HANDLE, preparsed_data: *mut isize) -> BOOLEAN;
    pub fn HidD_FreePreparsedData(preparsed_data: isize) -> BOOLEAN;
    pub fn HidP_GetCaps(preparsed_data: isize, capabilities: *mut HIDP_CAPS) -> i32;
    pub fn HidD_SetFeature(
        hid_device_object: HANDLE,
        report_buffer: *const c_void,
        report_buffer_length: u32,
    ) -> BOOLEAN;
    pub fn HidD_GetFeature(
        hid_device_object: HANDLE,
        report_buffer: *mut c_void,
        report_buffer_length: u32,
    ) -> BOOLEAN;
}

#[link(name = "setupapi")]
extern "system" {
    pub fn SetupDiGetClassDevsW(
        class_guid: *const GUID,
        enumerator: *const u16,
        hwnd_parent: *mut c_void,
        flags: u32,
    ) -> *mut c_void;
    pub fn SetupDiEnumDeviceInterfaces(
        device_info_set: *mut c_void,
        device_info_data: *mut c_void,
        interface_class_guid: *const GUID,
        member_index: u32,
        device_interface_data: *mut SP_DEVICE_INTERFACE_DATA,
    ) -> i32;
    pub fn SetupDiGetDeviceInterfaceDetailW(
        device_info_set: *mut c_void,
        device_interface_data: *mut SP_DEVICE_INTERFACE_DATA,
        device_interface_detail_data: *mut u8,
        device_interface_detail_data_size: u32,
        required_size: *mut u32,
        device_info_data: *mut c_void,
    ) -> i32;
    pub fn SetupDiDestroyDeviceInfoList(device_info_set: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut c_void,
    ) -> HANDLE;
    pub fn CloseHandle(object: HANDLE) -> i32;
    pub fn WriteFile(
        file: HANDLE,
        buffer: *const c_void,
        number_of_bytes_to_write: u32,
        number_of_bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn CreateEventW(
        lpEventAttributes: *mut c_void,
        bManualReset: i32,
        bInitialState: i32,
        lpName: *const u16,
    ) -> HANDLE;
    pub fn WaitForSingleObject(hHandle: HANDLE, dwMilliseconds: u32) -> u32;
    pub fn GetOverlappedResult(
        hFile: HANDLE,
        lpOverlapped: *mut c_void,
        lpNumberOfBytesTransferred: *mut u32,
        bWait: i32,
    ) -> i32;
    pub fn ResetEvent(hEvent: HANDLE) -> i32;
    pub fn CancelIoEx(hFile: HANDLE, lpOverlapped: *mut c_void) -> i32;
    pub fn QueryPerformanceCounter(lpPerformanceCount: *mut LARGE_INTEGER) -> i32;
    pub fn QueryPerformanceFrequency(lpFrequency: *mut LARGE_INTEGER) -> i32;
    pub fn GetLastError() -> u32;
    pub fn GetTickCount() -> u32;
    pub fn GetCurrentThread() -> HANDLE;
    pub fn SetThreadPriority(hThread: HANDLE, nPriority: i32) -> i32;
}

pub const THREAD_PRIORITY_ABOVE_NORMAL: i32 = 1;
pub const THREAD_PRIORITY_HIGHEST: i32 = 2;

#[link(name = "user32")]
extern "system" {
    pub fn GetLastInputInfo(plii: *mut LASTINPUTINFO) -> i32;
    pub fn GetAsyncKeyState(v_key: i32) -> i16;
}

/// Calculate system idle duration in milliseconds using GetLastInputInfo and GetTickCount
pub fn get_system_idle_ms() -> u64 {
    unsafe {
        let mut lii = LASTINPUTINFO {
            cb_size: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dw_time: 0,
        };
        if GetLastInputInfo(&mut lii) != 0 {
            let now = GetTickCount();
            if now >= lii.dw_time {
                (now - lii.dw_time) as u64
            } else {
                (u32::MAX - lii.dw_time + now) as u64
            }
        } else {
            0
        }
    }
}

/// Query whether a virtual key is currently pressed
#[inline]
pub fn is_key_down(v_key: i32) -> bool {
    unsafe { (GetAsyncKeyState(v_key) as u16 & 0x8000) != 0 }
}

// --- Windows Registry Helpers for OEM GamingCenter Compatibility Synchronization ---

pub fn set_hklm_dword(sub_key: &str, value_name: &str, value: u32) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
    };
    let sub_key_u16: Vec<u16> = sub_key.encode_utf16().chain(std::iter::once(0)).collect();
    let val_name_u16: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sub_key_u16.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        ).is_ok() {
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(val_name_u16.as_ptr()),
                0,
                REG_DWORD,
                Some(std::slice::from_raw_parts(&value as *const u32 as *const u8, 4)),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

pub fn set_hklm_sz(sub_key: &str, value_name: &str, value: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_SET_VALUE, REG_SZ, REG_OPTION_NON_VOLATILE,
    };
    let sub_key_u16: Vec<u16> = sub_key.encode_utf16().chain(std::iter::once(0)).collect();
    let val_name_u16: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
    let val_bytes_u16: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sub_key_u16.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        ).is_ok() {
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(val_name_u16.as_ptr()),
                0,
                REG_SZ,
                Some(std::slice::from_raw_parts(
                    val_bytes_u16.as_ptr() as *const u8,
                    val_bytes_u16.len() * 2,
                )),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

pub fn read_hklm_dword(sub_key: &str, value_name: &str) -> Option<u32> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_READ, REG_DWORD,
    };
    let sub_key_u16: Vec<u16> = sub_key.encode_utf16().chain(std::iter::once(0)).collect();
    let val_name_u16: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sub_key_u16.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        ).is_ok() {
            let mut data_type = REG_DWORD;
            let mut data = 0u32;
            let mut data_size = std::mem::size_of::<u32>() as u32;
            let res = RegQueryValueExW(
                hkey,
                PCWSTR(val_name_u16.as_ptr()),
                None,
                Some(&mut data_type),
                Some(&mut data as *mut u32 as *mut u8),
                Some(&mut data_size),
            );
            let _ = RegCloseKey(hkey);
            if res.is_ok() {
                return Some(data);
            }
        }
    }
    None
}
