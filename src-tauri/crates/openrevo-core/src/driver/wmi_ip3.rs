// OpenRevo Native Driver - Shenzhen IP3 (Century) ACPI-WMI Adapter
// Target Machine: Mechrevo Wujie 14 (IP3 xN39 Board, e.g. IP3_ARN39_MB_V21)
// Discovered Interfaces:
//   - root\WMI:PowerSwitchInterface (SetPowerMode, GetPowerMode)
//   - root\WMI:SuRwECRegInterface   (ReadECReg, WriteECReg)
//   - root\WMI:IP3_WMIEvent         (Fn+X mode switch notifications)
//
// Inviolable Rules:
//   - Pure ASCII only.
//   - Zero vendor dependency: 100% native Rust / Win32 WMI.
//   - STA thread isolation: COM calls isolated from WebView2 / Win32 main thread.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::LazyLock;
use std::thread;
use std::time::Duration;
use windows::core::{w, BSTR, VARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_PKT_PRIVACY, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, IWbemServices, WbemLocator,
    WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE,
};

const RPC_C_AUTHN_WINNT: u32 = 10;
const RPC_C_AUTHZ_NONE: u32 = 0;

static IP3_PROBED: AtomicBool = AtomicBool::new(false);
static IP3_AVAILABLE: AtomicBool = AtomicBool::new(false);

enum Ip3Task {
    GetPowerMode(Sender<Option<u8>>),
    SetPowerMode(u8, Sender<bool>),
    ReadEcReg(u32, Sender<Option<u8>>),
    WriteEcReg(u32, Sender<bool>),
}

struct Ip3Worker {
    tx: Sender<Ip3Task>,
}

static WORKER: LazyLock<Option<Ip3Worker>> = LazyLock::new(init_worker);

fn init_worker() -> Option<Ip3Worker> {
    let (tx, rx) = channel::<Ip3Task>();

    thread::Builder::new()
        .name("wmi-ip3-actor".to_string())
        .spawn(move || {
            worker_loop(rx);
        })
        .ok()?;

    Some(Ip3Worker { tx })
}

fn worker_loop(rx: Receiver<Ip3Task>) {
    unsafe {
        // Initialize COM exclusively within this dedicated background worker thread
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let mut services_opt: Option<IWbemServices> = None;
        let mut ps_instance_path: Option<BSTR> = None;
        let mut ps_class_obj: Option<IWbemClassObject> = None;
        let mut ec_instance_path: Option<BSTR> = None;
        let mut ec_class_obj: Option<IWbemClassObject> = None;
        let mut is_available = false;

        if let Ok(locator) = CoCreateInstance::<_, IWbemLocator>(&WbemLocator, None, CLSCTX_INPROC_SERVER) {
            let namespace = BSTR::from(r"root\WMI");
            if let Ok(services) = locator.ConnectServer(
                &namespace,
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                None,
            ) {
                // Ensure WMI proxy blanket impersonation and privacy for ACPI WMI kernel driver access
                let _ = CoSetProxyBlanket(
                    &services,
                    RPC_C_AUTHN_WINNT,
                    RPC_C_AUTHZ_NONE,
                    None,
                    RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
                    RPC_C_IMP_LEVEL_IMPERSONATE,
                    None,
                    EOAC_NONE,
                );

const CANDIDATE_POWER_SWITCH_INSTANCES: &[&str] = &[
    r"ACPI\PNP0C14\IP3POWERSWITCH",
    r"ACPI\PNP0C14\IP3POWERSWITCH_0",
    r"ACPI\PNP0C14\0_0",
];
const CANDIDATE_EC_REG_INSTANCES: &[&str] = &[
    r"ACPI\PNP0C14\RWECREGWMI",
    r"ACPI\PNP0C14\RWECREGWMI_0",
    r"ACPI\PNP0C14\0_0",
];

                // 1. Probe PowerSwitchInterface
                let q_lang = BSTR::from("WQL");
                let q_str = BSTR::from("SELECT __RELPATH, InstanceName FROM PowerSwitchInterface");
                if let Ok(enumerator) = services.ExecQuery(
                    &q_lang,
                    &q_str,
                    WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                    None,
                ) {
                    let mut objects = [None];
                    let mut returned = 0;
                    if enumerator.Next(2000, &mut objects, &mut returned).is_ok() && returned > 0 {
                        if let Some(obj) = objects[0].take() {
                            let mut val_rel = VARIANT::default();
                            let rel_path = if obj.Get(w!("__RELPATH"), 0, &mut val_rel, None, None).is_ok() {
                                val_rel.to_string()
                            } else {
                                String::new()
                            };

                            let mut val = VARIANT::default();
                            let inst_str = if obj.Get(w!("InstanceName"), 0, &mut val, None, None).is_ok() {
                                val.to_string()
                            } else {
                                String::new()
                            };

                            let path_to_use = if !rel_path.is_empty() {
                                rel_path
                            } else if !inst_str.is_empty() {
                                let escaped = inst_str.replace('\\', "\\\\");
                                format!("PowerSwitchInterface.InstanceName=\"{}\"", escaped)
                            } else {
                                String::new()
                            };

                            if !path_to_use.is_empty() {
                                let cls_name = BSTR::from("PowerSwitchInterface");
                                let mut co: Option<IWbemClassObject> = None;
                                if services.GetObject(&cls_name, WBEM_GENERIC_FLAG_TYPE(0), None, Some(&mut co), None).is_ok() {
                                    ps_instance_path = Some(BSTR::from(path_to_use));
                                    ps_class_obj = co;
                                    is_available = true;
                                    crate::log_info!("WMI_IP3", "Connected to PowerSwitchInterface: {}", inst_str);
                                }
                            }
                        }
                    }
                }
                if ps_instance_path.is_none() {
                    for cand in CANDIDATE_POWER_SWITCH_INSTANCES {
                        let escaped = cand.replace('\\', "\\\\");
                        let candidate_path =
                            BSTR::from(format!("PowerSwitchInterface.InstanceName=\"{}\"", escaped));
                        let mut instance_obj: Option<IWbemClassObject> = None;
                        if services
                            .GetObject(
                                &candidate_path,
                                WBEM_GENERIC_FLAG_TYPE(0),
                                None,
                                Some(&mut instance_obj),
                                None,
                            )
                            .is_err()
                        {
                            continue;
                        }

                        let cls_name = BSTR::from("PowerSwitchInterface");
                        let mut co: Option<IWbemClassObject> = None;
                        if services.GetObject(&cls_name, WBEM_GENERIC_FLAG_TYPE(0), None, Some(&mut co), None).is_ok() {
                            ps_instance_path = Some(candidate_path);
                            ps_class_obj = co;
                            is_available = true;
                            crate::log_info!("WMI_IP3", "Bound to candidate PowerSwitchInterface: {}", cand);
                            break;
                        }
                    }
                }
                if ps_instance_path.is_none() {
                    crate::log_warn!("WMI_IP3", "No bindable PowerSwitchInterface instance found");
                }

                // 2. Probe SuRwECRegInterface
                let q_ec = BSTR::from("SELECT __RELPATH, InstanceName FROM SuRwECRegInterface");
                if let Ok(enumerator) = services.ExecQuery(
                    &q_lang,
                    &q_ec,
                    WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                    None,
                ) {
                    let mut objects = [None];
                    let mut returned = 0;
                    if enumerator.Next(2000, &mut objects, &mut returned).is_ok() && returned > 0 {
                        if let Some(obj) = objects[0].take() {
                            let mut val_rel = VARIANT::default();
                            let rel_path = if obj.Get(w!("__RELPATH"), 0, &mut val_rel, None, None).is_ok() {
                                val_rel.to_string()
                            } else {
                                String::new()
                            };

                            let mut val = VARIANT::default();
                            let inst_str = if obj.Get(w!("InstanceName"), 0, &mut val, None, None).is_ok() {
                                val.to_string()
                            } else {
                                String::new()
                            };

                            let path_to_use = if !rel_path.is_empty() {
                                rel_path
                            } else if !inst_str.is_empty() {
                                let escaped = inst_str.replace('\\', "\\\\");
                                format!("SuRwECRegInterface.InstanceName=\"{}\"", escaped)
                            } else {
                                String::new()
                            };

                            if !path_to_use.is_empty() {
                                let cls_name = BSTR::from("SuRwECRegInterface");
                                let mut co: Option<IWbemClassObject> = None;
                                if services.GetObject(&cls_name, WBEM_GENERIC_FLAG_TYPE(0), None, Some(&mut co), None).is_ok() {
                                    ec_instance_path = Some(BSTR::from(path_to_use));
                                    ec_class_obj = co;
                                    crate::log_info!("WMI_IP3", "Connected to SuRwECRegInterface: {}", inst_str);
                                }
                            }
                        }
                    }
                }
                if ec_instance_path.is_none() {
                    for cand in CANDIDATE_EC_REG_INSTANCES {
                        let escaped = cand.replace('\\', "\\\\");
                        let candidate_path =
                            BSTR::from(format!("SuRwECRegInterface.InstanceName=\"{}\"", escaped));
                        let mut instance_obj: Option<IWbemClassObject> = None;
                        if services
                            .GetObject(
                                &candidate_path,
                                WBEM_GENERIC_FLAG_TYPE(0),
                                None,
                                Some(&mut instance_obj),
                                None,
                            )
                            .is_err()
                        {
                            continue;
                        }

                        let cls_name = BSTR::from("SuRwECRegInterface");
                        let mut co: Option<IWbemClassObject> = None;
                        if services.GetObject(&cls_name, WBEM_GENERIC_FLAG_TYPE(0), None, Some(&mut co), None).is_ok() {
                            ec_instance_path = Some(candidate_path);
                            ec_class_obj = co;
                            crate::log_info!("WMI_IP3", "Bound to candidate SuRwECRegInterface: {}", cand);
                            break;
                        }
                    }
                }

                if is_available {
                    services_opt = Some(services);
                }
            }
        }

        IP3_AVAILABLE.store(is_available, Ordering::Release);
        IP3_PROBED.store(true, Ordering::Release);
        crate::log_info!("WMI_IP3", "IP3 WMI initialization completed: is_available={}", is_available);

        while let Ok(task) = rx.recv() {
            match task {
                Ip3Task::GetPowerMode(resp) => {
                    let mut res = None;
                    if is_available {
                        res = (|| -> Option<u8> {
                            let services = services_opt.as_ref()?;
                            let path = ps_instance_path.as_ref()?;
                            let method = BSTR::from("GetPowerMode");

                            let mut out_params: Option<IWbemClassObject> = None;
                            services.ExecMethod(
                                path,
                                &method,
                                WBEM_GENERIC_FLAG_TYPE(0),
                                None,
                                None,
                                Some(&mut out_params),
                                None,
                            ).ok()?;

                            let out = out_params?;
                            let mut ret_val = VARIANT::default();
                            out.Get(w!("CurrentPowerMode"), 0, &mut ret_val, None, None).ok()?;
                            extract_variant_u8(&ret_val)
                        })();
                    }
                    let _ = resp.send(res);
                }
                Ip3Task::SetPowerMode(raw_mode, resp) => {
                    let mut ok = false;
                    if is_available {
                        ok = (|| -> Option<bool> {
                            let services = services_opt.as_ref()?;
                            let path = ps_instance_path.as_ref()?;
                            let class_obj = ps_class_obj.as_ref()?;

                            let mut in_sig: Option<IWbemClassObject> = None;
                            class_obj.GetMethod(w!("SetPowerMode"), 0, &mut in_sig, std::ptr::null_mut()).ok()?;
                            let in_params = in_sig?.SpawnInstance(0).ok()?;

                            // 1. First attempt: VT_UI1 (byte)
                            let mut var_mode = VARIANT::from(raw_mode);
                            let _ = in_params.Put(w!("PowerMode"), 0, &mut var_mode, 0);

                            let method = BSTR::from("SetPowerMode");
                            let mut out_params: Option<IWbemClassObject> = None;
                            let mut hr = services.ExecMethod(
                                path,
                                &method,
                                WBEM_GENERIC_FLAG_TYPE(0),
                                None,
                                &in_params,
                                Some(&mut out_params),
                                None,
                            );

                            // 2. Fallback attempt: VT_UI4 (uint32)
                            if hr.is_err() {
                                let mut var_mode32 = VARIANT::from(raw_mode as u32);
                                let _ = in_params.Put(w!("PowerMode"), 0, &mut var_mode32, 0);
                                hr = services.ExecMethod(
                                    path,
                                    &method,
                                    WBEM_GENERIC_FLAG_TYPE(0),
                                    None,
                                    &in_params,
                                    Some(&mut out_params),
                                    None,
                                );
                            }

                            crate::log_info!("WMI_IP3", "SetPowerMode({}) hr={:?}", raw_mode, hr);
                            Some(hr.is_ok())
                        })().unwrap_or(false);
                    }
                    let _ = resp.send(ok);
                }
                Ip3Task::ReadEcReg(packed_addr, resp) => {
                    let mut res = None;
                    if is_available && ec_instance_path.is_some() {
                        res = (|| -> Option<u8> {
                            let services = services_opt.as_ref()?;
                            let path = ec_instance_path.as_ref()?;
                            let class_obj = ec_class_obj.as_ref()?;

                            let mut in_sig: Option<IWbemClassObject> = None;
                            class_obj.GetMethod(w!("ReadECReg"), 0, &mut in_sig, std::ptr::null_mut()).ok()?;
                            let in_params = in_sig?.SpawnInstance(0).ok()?;

                            let mut var_param = VARIANT::from(packed_addr);
                            in_params.Put(w!("GroupOffset"), 0, &mut var_param, 0).ok()?;

                            let method = BSTR::from("ReadECReg");
                            let mut out_params: Option<IWbemClassObject> = None;
                            services.ExecMethod(
                                path,
                                &method,
                                WBEM_GENERIC_FLAG_TYPE(0),
                                None,
                                &in_params,
                                Some(&mut out_params),
                                None,
                            ).ok()?;

                            let out = out_params?;
                            let mut ret_val = VARIANT::default();
                            out.Get(w!("uStringReturn"), 0, &mut ret_val, None, None).ok()?;
                            let out_str = ret_val.to_string();
                            let parts: Vec<&str> = out_str.split(',').collect();
                            if parts.len() >= 2 {
                                let hex_str = parts[1].trim().trim_start_matches("0x").trim_start_matches("0X");
                                u8::from_str_radix(hex_str, 16).ok()
                            } else {
                                None
                            }
                        })();
                    }
                    let _ = resp.send(res);
                }
                Ip3Task::WriteEcReg(packed_val, resp) => {
                    let mut ok = false;
                    if is_available && ec_instance_path.is_some() {
                        ok = (|| -> Option<bool> {
                            let services = services_opt.as_ref()?;
                            let path = ec_instance_path.as_ref()?;
                            let class_obj = ec_class_obj.as_ref()?;

                            let mut in_sig: Option<IWbemClassObject> = None;
                            class_obj.GetMethod(w!("WriteECReg"), 0, &mut in_sig, std::ptr::null_mut()).ok()?;
                            let sig = in_sig?;
                            let in_params = sig.SpawnInstance(0).ok()?;

                            // 1. Primary COM attempt: VT_UI4 (uint32)
                            let mut var_param = VARIANT::from(packed_val);
                            let _ = in_params.Put(w!("GroupOffsetValue"), 0, &mut var_param, 0);
                            let method = BSTR::from("WriteECReg");
                            let mut out_params: Option<IWbemClassObject> = None;
                            let mut hr = services.ExecMethod(
                                path,
                                &method,
                                WBEM_GENERIC_FLAG_TYPE(0),
                                None,
                                &in_params,
                                Some(&mut out_params),
                                None,
                            );

                            // 2. Secondary COM attempt: VT_I4 (int32 fallback)
                            if hr.is_err() {
                                let mut var_param_i32 = VARIANT::from(packed_val as i32);
                                let _ = in_params.Put(w!("GroupOffsetValue"), 0, &mut var_param_i32, 0);
                                hr = services.ExecMethod(
                                    path,
                                    &method,
                                    WBEM_GENERIC_FLAG_TYPE(0),
                                    None,
                                    &in_params,
                                    Some(&mut out_params),
                                    None,
                                );
                            }

                            crate::log_info!("WMI_IP3", "Native COM WriteECReg(0x{:08X}) hr={:?}", packed_val, hr);
                            let write_ok = hr.is_ok();
                            if write_ok {
                                if let Some(out) = out_params {
                                    let mut ret_val = VARIANT::default();
                                    if out.Get(w!("uStringReturn"), 0, &mut ret_val, None, None).is_ok() {
                                        crate::log_info!("WMI_IP3", "WriteECReg result: {}", ret_val.to_string());
                                    }
                                }
                            }

                            // 3. Chain WriteReadECReg: In vendor IP3 ACPI-WMI, WriteReadECReg serves as
                            // the hardware latch/commit method that triggers the EC to refresh the active register state.
                            let mut in_sig2: Option<IWbemClassObject> = None;
                            if class_obj.GetMethod(w!("WriteReadECReg"), 0, &mut in_sig2, std::ptr::null_mut()).is_ok() {
                                if let Some(sig2) = in_sig2 {
                                    if let Ok(in_params2) = sig2.SpawnInstance(0) {
                                        let mut var_param = VARIANT::from(packed_val);
                                        let _ = in_params2.Put(w!("GroupOffsetValue"), 0, &mut var_param, 0);
                                        let method2 = BSTR::from("WriteReadECReg");
                                        let mut out_params2: Option<IWbemClassObject> = None;
                                        let hr2 = services.ExecMethod(
                                            path,
                                            &method2,
                                            WBEM_GENERIC_FLAG_TYPE(0),
                                            None,
                                            &in_params2,
                                            Some(&mut out_params2),
                                            None,
                                        );
                                        if hr2.is_ok() {
                                            crate::log_info!("WMI_IP3", "Committed WriteReadECReg(0x{:08X}) succeeded", packed_val);
                                            return Some(true);
                                        }
                                    }
                                }
                            }

                            if write_ok {
                                return Some(true);
                            }

                            None
                        })().unwrap_or(false);
                    }
                    let _ = resp.send(ok);
                }
            }
        }

        CoUninitialize();
    }
}

/// Probes whether Shenzhen IP3 ACPI-WMI is available on this machine
pub fn probe_ip3_available() -> bool {
    if !IP3_PROBED.load(Ordering::Acquire) {
        let _ = WORKER.as_ref();
        let start = std::time::Instant::now();
        while !IP3_PROBED.load(Ordering::Acquire) && start.elapsed().as_millis() < 4000 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    IP3_AVAILABLE.load(Ordering::Acquire)
}

pub fn is_ip3_available() -> bool {
    probe_ip3_available()
}

/// Read performance mode from root\WMI:PowerSwitchInterface.GetPowerMode()
/// Returns raw vendor code:
///   2 = Silence (Office)
///   0 = Balance (Balance)
///   1 = Rage (Turbo)
pub fn ip3_get_power_mode() -> Option<u8> {
    if !is_ip3_available() {
        return None;
    }
    let worker = WORKER.as_ref()?;
    let (tx, rx) = channel();
    worker.tx.send(Ip3Task::GetPowerMode(tx)).ok()?;
    rx.recv_timeout(Duration::from_millis(1500)).ok().flatten()
}

/// Set performance mode via root\WMI:PowerSwitchInterface.SetPowerMode(raw_mode)
/// Raw mode mapping:
///   Office  -> 2
///   Balance -> 0
///   Turbo   -> 1
pub fn ip3_set_power_mode(raw_mode: u8) -> bool {
    if !is_ip3_available() {
        return false;
    }
    let worker = match WORKER.as_ref() {
        Some(w) => w,
        None => return false,
    };
    let (tx, rx) = channel();
    if worker.tx.send(Ip3Task::SetPowerMode(raw_mode, tx)).is_err() {
        return false;
    }
    rx.recv_timeout(Duration::from_millis(1500)).unwrap_or(false)
}

/// Read EC register via SuRwECRegInterface
/// Packing formula: group | (offset << 8)
pub fn ip3_read_ec_reg(group: u8, offset: u8) -> Option<u8> {
    if !is_ip3_available() {
        return None;
    }
    let worker = WORKER.as_ref()?;
    let (tx, rx) = channel();
    let packed = (group as u32) | ((offset as u32) << 8);
    worker.tx.send(Ip3Task::ReadEcReg(packed, tx)).ok()?;
    rx.recv_timeout(Duration::from_millis(1500)).ok().flatten()
}

/// Write EC register via SuRwECRegInterface
/// Packing formula: group | (offset << 8) | (value << 16)
pub fn ip3_write_ec_reg(group: u8, offset: u8, value: u8) -> bool {
    if !is_ip3_available() {
        return false;
    }
    let worker = match WORKER.as_ref() {
        Some(w) => w,
        None => return false,
    };
    let (tx, rx) = channel();
    let packed = (group as u32) | ((offset as u32) << 8) | ((value as u32) << 16);
    if worker.tx.send(Ip3Task::WriteEcReg(packed, tx)).is_err() {
        return false;
    }
    rx.recv_timeout(Duration::from_millis(1500)).unwrap_or(false)
}

/// Keyboard Backlight controller for IP3 board
/// EC Group 3, Offset 0xE2
/// Values:
///   0x00 = Off
///   0x5C = Low
///   0xB8 = High
pub const IP3_BL_GROUP: u8 = 3;
pub const IP3_BL_OFFSET: u8 = 0xE2;

pub fn ip3_set_keyboard_backlight_level(level_idx: u8) -> bool {
    let raw_val = match level_idx {
        0 => 0x00,
        1 => 0x5C,
        2..=4 => 0xB8,
        _ => 0x5C,
    };
    let ok = ip3_write_ec_reg(IP3_BL_GROUP, IP3_BL_OFFSET, raw_val);
    if ok {
        // Attempt to pulse an inert software keystroke (0xE8) to nudge the keyboard controller / OS input
        // to immediately latch the new PWM duty cycle without waiting for physical user keystroke.
        unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
            };
            keybd_event(0xE8, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
        }
    }
    crate::log_info!(
        "WMI_IP3",
        "Set keyboard backlight level_idx={} (EC val=0x{:02X}) -> ok={}",
        level_idx,
        raw_val,
        ok
    );
    ok
}

pub fn ip3_get_keyboard_backlight_level() -> Option<u8> {
    let raw = ip3_read_ec_reg(IP3_BL_GROUP, IP3_BL_OFFSET)?;
    match raw {
        0x00 => Some(0),
        0x5C => Some(1),
        0xB8 => Some(2),
        _ => Some(1),
    }
}

/// Helper function to safely extract u8 from VARIANT (supports integers and string representation)
fn extract_variant_u8(val: &VARIANT) -> Option<u8> {
    let s = val.to_string();
    if let Ok(num) = s.trim().parse::<u8>() {
        return Some(num);
    }
    // COM VARIANT memory inspection fallback
    unsafe {
        let raw_ptr = val as *const VARIANT as *const u16;
        let vt = *raw_ptr;
        let data_ptr = (val as *const VARIANT as *const u8).add(8);
        match vt {
            16 => Some(*data_ptr),                        // VT_I1
            17 => Some(*data_ptr),                        // VT_UI1
            2 => Some(*(data_ptr as *const i16) as u8),   // VT_I2
            18 => Some(*(data_ptr as *const u16) as u8),  // VT_UI2
            3 => Some(*(data_ptr as *const i32) as u8),   // VT_I4
            19 => Some(*(data_ptr as *const u32) as u8),  // VT_UI4
            _ => None,
        }
    }
}

