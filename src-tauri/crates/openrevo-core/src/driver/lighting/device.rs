// OpenRevo - ITE Lighting Device Handle & Win32 HID Enumeration
// Manages safe handles, feature reports, and device discovery.

use std::ffi::c_void;
use std::thread::sleep;
use std::time::Duration;
use crate::driver::hid_ffi::*;
use crate::hal::vault::ite::*;

pub struct IteLightingDevice {
    handle: HANDLE,
    pub usage: u16,
    pub usage_page: u16,
}

unsafe impl Send for IteLightingDevice {}
unsafe impl Sync for IteLightingDevice {}

impl Drop for IteLightingDevice {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE && !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
            self.handle = INVALID_HANDLE_VALUE;
        }
    }
}

impl IteLightingDevice {
    pub fn write_feature(&self, report: &[u8]) -> bool {
        if self.handle == INVALID_HANDLE_VALUE || self.handle.is_null() {
            return false;
        }
        let res = unsafe {
            HidD_SetFeature(
                self.handle,
                report.as_ptr() as *const c_void,
                report.len() as u32,
            )
        };
        let is_flash_write = report.len() >= 9 && (report[1] == 8 || report[1] == 20) && report[8] == 1;
        let sleep_ms = if is_flash_write { 50 } else { 20 };
        // Safe pacing interval to prevent ITE firmware USB FIFO overrun and eFlash collision
        sleep(Duration::from_millis(sleep_ms));
        res != 0
    }

    pub fn get_feature(&self, report: &mut [u8]) -> bool {
        if self.handle == INVALID_HANDLE_VALUE || self.handle.is_null() {
            return false;
        }
        let res = unsafe {
            HidD_GetFeature(
                self.handle,
                report.as_mut_ptr() as *mut c_void,
                report.len() as u32,
            )
        };
        res != 0
    }

    /// Probe ITE firmware capability report byte 2 via feature report [0, 162, 0, 0, 0, 0, 0, 0, 0]
    pub fn probe_capability(&self) -> Option<u8> {
        let mut report = [0u8; 9];
        report[1] = 162; // 0xA2 query packet
        if self.write_feature(&report) {
            let mut read_buf = [0u8; 9];
            if self.get_feature(&mut read_buf) {
                return Some(read_buf[2]);
            }
        }
        None
    }

    /// Query ITE firmware version bytes via 80H feature report [0, 0x80, 0, 0, 0, 0, 0, 0, 0]
    /// Returns (Ver_High, Ver_Low, Ver_Test, Ver_Customer) as defined in official LM_ITE_RGB.cs
    pub fn query_firmware_version_80h(&self) -> Option<(u8, u8, u8, u8)> {
        let mut report = [0u8; 9];
        report[1] = 0x80;
        if self.write_feature(&report) {
            let mut read_buf = [0u8; 9];
            if self.get_feature(&mut read_buf) {
                return Some((read_buf[2], read_buf[3], read_buf[4], read_buf[5]));
            }
        }
        None
    }

    pub fn open(vid: u16, pid: u16, target_usage: u16) -> Option<Self> {
        Self::open_internal(vid, &[pid], target_usage, None)
    }

    pub fn open_with_pids(vid: u16, pids: &[u16], target_usage: u16) -> Option<Self> {
        Self::open_internal(vid, pids, target_usage, None)
    }

    pub fn open_exact(vid: u16, pid: u16, target_usage: u16, target_usage_page: u16) -> Option<Self> {
        Self::open_internal(vid, &[pid], target_usage, Some(target_usage_page))
    }

    fn open_internal(vid: u16, pids: &[u16], target_usage: u16, target_usage_page: Option<u16>) -> Option<Self> {
        unsafe {
            let mut hid_guid = GUID::default();
            HidD_GetHidGuid(&mut hid_guid);

            let dev_info = SetupDiGetClassDevsW(
                &hid_guid,
                std::ptr::null(),
                std::ptr::null_mut(),
                DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
            );
            if dev_info == INVALID_HANDLE_VALUE {
                return None;
            }

            let mut iface_data = SP_DEVICE_INTERFACE_DATA {
                cb_size: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                interface_class_guid: GUID::default(),
                flags: 0,
                reserved: 0,
            };

            let mut index = 0;
            while SetupDiEnumDeviceInterfaces(
                dev_info,
                std::ptr::null_mut(),
                &hid_guid,
                index,
                &mut iface_data,
            ) != 0
            {
                index += 1;

                let mut req_size: u32 = 0;
                SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &mut iface_data,
                    std::ptr::null_mut(),
                    0,
                    &mut req_size,
                    std::ptr::null_mut(),
                );

                if req_size == 0 {
                    continue;
                }

                let mut detail_buffer: Vec<u8> = vec![0u8; req_size as usize];
                let cb_size_field = if std::mem::size_of::<usize>() == 8 {
                    8u32
                } else {
                    6u32
                };
                *(detail_buffer.as_mut_ptr() as *mut u32) = cb_size_field;

                if SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &mut iface_data,
                    detail_buffer.as_mut_ptr(),
                    req_size,
                    &mut req_size,
                    std::ptr::null_mut(),
                ) != 0
                {
                    let device_path_ptr = detail_buffer.as_ptr().add(4) as *const u16;

                    // Inspect path string BEFORE CreateFileW to prevent hang/slowdown
                    let mut path_len = 0;
                    while *device_path_ptr.add(path_len) != 0 {
                        path_len += 1;
                    }
                    let path_str = String::from_utf16_lossy(std::slice::from_raw_parts(device_path_ptr, path_len)).to_lowercase();
                    let target_vid_str = format!("vid_{:04x}", vid);

                    if !path_str.contains(&target_vid_str) {
                        continue;
                    }

                    let mut matched_pid = None;
                    for &p in pids {
                        let target_pid_str = format!("pid_{:04x}", p);
                        if path_str.contains(&target_pid_str) {
                            matched_pid = Some(p);
                            break;
                        }
                    }

                    if matched_pid.is_none() {
                        continue;
                    }
                    let pid = matched_pid.unwrap();

                    let handle = CreateFileW(
                        device_path_ptr,
                        GENERIC_READ | GENERIC_WRITE,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        std::ptr::null_mut(),
                        OPEN_EXISTING,
                        FILE_ATTRIBUTE_NORMAL,
                        std::ptr::null_mut(),
                    );

                    if handle != INVALID_HANDLE_VALUE && !handle.is_null() {
                        let mut attrs = HIDD_ATTRIBUTES {
                            size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                            vendor_id: 0,
                            product_id: 0,
                            version_number: 0,
                        };

                        if HidD_GetAttributes(handle, &mut attrs) != 0 {
                            if attrs.vendor_id == vid && attrs.product_id == pid {
                                let mut preparsed: isize = 0;
                                if HidD_GetPreparsedData(handle, &mut preparsed) != 0 {
                                    let mut caps: HIDP_CAPS = std::mem::zeroed();
                                    if HidP_GetCaps(preparsed, &mut caps) == 0x00110000 {
                                        let is_valid_page = if let Some(target_page) = target_usage_page {
                                            caps.usage_page == target_page
                                        } else {
                                            caps.usage_page == USAGE_PAGE_ITE
                                                || caps.usage_page == USAGE_PAGE_ME_1ST
                                                || caps.usage_page == USAGE_PAGE_FOURZONE
                                        };
                                        if is_valid_page && (target_usage == 0 || caps.usage == target_usage) {
                                            HidD_FreePreparsedData(preparsed);
                                            SetupDiDestroyDeviceInfoList(dev_info);
                                            return Some(IteLightingDevice {
                                                handle,
                                                usage: caps.usage,
                                                usage_page: caps.usage_page,
                                            });
                                        }
                                    }
                                    HidD_FreePreparsedData(preparsed);
                                }
                            }
                        }
                        CloseHandle(handle);
                    }
                }
            }

            SetupDiDestroyDeviceInfoList(dev_info);
            None
        }
    }
}
