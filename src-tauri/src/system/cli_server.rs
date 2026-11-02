// Command-line control channel for `orv.exe`.
// A named pipe (\\.\pipe\openrevo-cli) that only this Windows user, Administrators and SYSTEM may open.
// One JSON request line in ({"cmd": "...", "args": {...}}), one JSON response out, then disconnect.
// Every command goes through the same functions the UI buttons call, so the EC cache, config and
// the open panel stay consistent. `invoke` reaches every IPC command the panel itself can call
// (and nothing else: the name must be in the build-time catalog of registered commands).

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::driver::ec_bridge::SharedEcBridge;
use crate::driver::gpu_bridge::SharedGpuBridge;

pub const PIPE_NAME: &str = r"\\.\pipe\openrevo-cli";

pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("openrevo-cli-pipe".into())
        .spawn(move || serve(app))
        .ok();
}

fn mode_name(m: u8) -> &'static str {
    match m {
        1 => "office",
        2 => "balanced",
        3 => "turbo",
        4 => "custom",
        _ => "unknown",
    }
}

fn parse_mode(s: &str) -> Option<u8> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "office" | "办公" => Some(1),
        "2" | "balanced" | "balance" | "均衡" => Some(2),
        "3" | "turbo" | "狂暴" => Some(3),
        "4" | "custom" | "自定义" => Some(4),
        _ => None,
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(|v| v.as_str()).ok_or_else(|| format!("missing argument '{}'", key))
}

fn arg_bool(args: &Value, key: &str) -> Result<bool, String> {
    args.get(key).and_then(|v| v.as_bool()).ok_or_else(|| format!("missing argument '{}'", key))
}

/// f32 -> one decimal without float noise in the JSON (36.9, not 36.900001525878906).
fn round1(x: f32) -> f64 {
    (x as f64 * 10.0).round() / 10.0
}

fn status(app: &AppHandle, args: &Value) -> Value {
    let ec = app.state::<SharedEcBridge>();
    let gpu = app.state::<SharedGpuBridge>();
    let cfg = crate::system::config::load_config();
    let s = crate::commands::system_cmd::collect_hardware_state(ec.inner(), gpu.inner(), false);
    let (bat, _, charging) = crate::commands::power_cmd::get_live_power_status();
    // Never wake a sleeping dGPU in hybrid mode unless asked (--gpu).
    let want_gpu = cfg.gpu_mode == "dgpu" || args.get("gpu").and_then(|v| v.as_bool()).unwrap_or(false);
    let gpu_json = if want_gpu {
        match crate::driver::nvml::sample() {
            Some(g) => json!({
                "temp_c": g.temp_c, "power_w": round1(g.power_w), "limit_w": g.power_limit_w.round() as u32,
                "core_mhz": g.core_mhz, "mem_mhz": g.mem_mhz, "usage_pct": g.usage_pct,
                "vram_used_mb": g.vram_used_mb, "vram_total_mb": g.vram_total_mb,
                "throttle": crate::driver::nvml::throttle_label(g.throttle_reasons),
            }),
            None => Value::Null,
        }
    } else {
        Value::Null
    };
    let profile = crate::system::config::list_profiles()
        .into_iter()
        .find(|p| p.id == cfg.active_profile_id)
        .map(|p| p.name)
        .unwrap_or_default();
    let fan = crate::commands::fan_cmd::get_fan_apply_status();
    let mut mem = windows::Win32::System::SystemInformation::MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<windows::Win32::System::SystemInformation::MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let ram_used_gb = unsafe {
        if windows::Win32::System::SystemInformation::GlobalMemoryStatusEx(&mut mem).is_ok() {
            ((mem.ullTotalPhys - mem.ullAvailPhys) as f64 / 1073741824.0 * 10.0).round() / 10.0
        } else {
            0.0
        }
    };
    json!({
        "mode": mode_name(s.power_mode), "mode_id": s.power_mode, "custom_profile": profile,
        "fan_boost": s.fan_boost, "fan_policy": cfg.preset_fan_policy,
        "fan_table": fan.state, "fan_table_source": fan.source,
        "cpu": { "temp_c": s.cpu_temp, "power_w": round1(s.cpu_power_w), "freq_mhz": s.cpu_freq, "usage_pct": s.cpu_usage },
        "gpu": gpu_json, "gpu_mode": cfg.gpu_mode,
        "fans_rpm": { "cpu": s.cpu_fan_rpm, "gpu": s.gpu_fan_rpm },
        "ram_used_gb": ram_used_gb,
        "battery": { "percent": bat, "ac": s.is_dc_online, "charging": charging },
        "refresh_hz": s.refresh_rate, "refresh_available": s.available_refresh_rates,
        "keep_awake": crate::system::switches::is_keep_awake(),
        "screen_off": crate::system::switches::is_screen_off(),
        "rtss_osd": cfg.rtss_osd_enabled,
        "win_key_locked": s.win_key_locked,
    })
}

/// Push a fresh state to the panel after a change made from the command line.
fn refresh_ui(app: &AppHandle) {
    let ec = app.state::<SharedEcBridge>();
    let gpu = app.state::<SharedGpuBridge>();
    let state = crate::commands::system_cmd::collect_hardware_state(
        ec.inner(),
        gpu.inner(),
        crate::system::telemetry::should_query_gpu(),
    );
    let _ = app.emit("hardware-state-updated", &state);
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn dispatch(app: &AppHandle, cmd: &str, args: &Value) -> Result<Value, String> {
    use crate::commands::*;
    let ec = || app.state::<SharedEcBridge>();
    let gpu = || app.state::<SharedGpuBridge>();
    let changed = |v: Value| -> Result<Value, String> {
        refresh_ui(app);
        Ok(v)
    };
    match cmd {
        "ping" => Ok(json!({ "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id() })),
        "catalog" => Ok(serde_json::from_str(catalog::COMMAND_CATALOG).unwrap_or(Value::Null)),
        "invoke" => {
            let command = arg_str(args, "command")?.to_string();
            let known: Value = serde_json::from_str(catalog::COMMAND_CATALOG).unwrap_or(Value::Null);
            if !known.as_array().map_or(false, |a| a.iter().any(|c| c["name"] == command)) {
                return Err(format!("unknown command '{}' (see `orv commands`)", command));
            }
            let payload = args.get("args").cloned().unwrap_or_else(|| json!({}));
            let result = invoke_like_frontend(app, &command, payload)?;
            if !command.starts_with("get_") && !command.starts_with("read_") && !command.starts_with("is_") {
                refresh_ui(app);
            }
            Ok(result)
        }
        "status" => Ok(status(app, args)),

        "mode" => {
            let mode = parse_mode(arg_str(args, "mode")?).ok_or("mode must be office | balanced | turbo | custom")?;
            if let Some(p) = args.get("profile").and_then(|v| v.as_str()) {
                if mode != 4 {
                    return Err("a profile can only be given with 'custom'".into());
                }
                let profiles = crate::system::config::list_profiles();
                let target = profiles
                    .iter()
                    .find(|x| x.id == p)
                    .or_else(|| profiles.iter().find(|x| x.name == p))
                    .or_else(|| profiles.iter().find(|x| x.name.to_lowercase().contains(&p.to_lowercase())))
                    .ok_or_else(|| format!("no custom profile matches '{}' (see `orv profile list`)", p))?;
                tuning_cmd::set_active_profile_id(target.id.clone(), ec())?;
            }
            power_cmd::set_power_mode(mode, app.clone(), ec())?;
            changed(json!({ "mode": mode_name(mode) }))
        }
        "boost" => {
            let on = arg_bool(args, "on")?;
            power_cmd::set_fan_boost(app.clone(), on, ec())?;
            changed(json!({ "fan_boost": on }))
        }
        "fan" => Ok(json!({
            "apply": to_value(fan_cmd::get_fan_apply_status()),
            "policy": crate::system::config::load_config().preset_fan_policy,
        })),
        "fan_policy" => {
            let policy = arg_str(args, "policy")?.to_string();
            fan_cmd::set_preset_fan_policy(policy.clone())?;
            changed(json!({ "fan_policy": policy }))
        }
        "profiles" => {
            let active = crate::system::config::load_config().active_profile_id;
            Ok(Value::Array(
                crate::system::config::list_profiles()
                    .into_iter()
                    .map(|p| json!({ "id": p.id, "name": p.name, "builtin": p.is_builtin, "active": p.id == active }))
                    .collect(),
            ))
        }
        "gpu" => match args.get("mode").and_then(|v| v.as_str()) {
            None => Ok(to_value(tuning_cmd::get_gpu_mode_info(gpu()))),
            Some(m) => {
                if !matches!(m, "hybrid" | "dgpu" | "igpu") {
                    return Err("gpu mode must be hybrid | dgpu | igpu".into());
                }
                if !args.get("yes").and_then(|v| v.as_bool()).unwrap_or(false) {
                    return Err("switching the GPU mode needs a reboot; repeat with --yes".into());
                }
                let reboot = args.get("reboot").and_then(|v| v.as_bool()).unwrap_or(false);
                let info = tuning_cmd::set_gpu_mode(app.clone(), m.to_string(), Some(reboot), gpu())?;
                changed(to_value(info))
            }
        },
        "hz" => {
            let hz = args.get("hz").and_then(|v| v.as_u64()).ok_or("missing argument 'hz'")? as u32;
            let s = system_cmd::collect_hardware_state(ec().inner(), gpu().inner(), false);
            if !s.available_refresh_rates.is_empty() && !s.available_refresh_rates.contains(&hz) {
                return Err(format!("{} Hz not offered by the panel (available: {:?})", hz, s.available_refresh_rates));
            }
            display_cmd::switch_refresh_rate(hz)?;
            changed(json!({ "refresh_hz": hz }))
        }
        "osd" => {
            let on = arg_bool(args, "on")?;
            shortcut_cmd::set_rtss_osd_enabled(on);
            Ok(json!({ "rtss_osd": on }))
        }
        "switch" => {
            let name = arg_str(args, "name")?;
            if !matches!(name, "wifi" | "bluetooth" | "touchpad" | "camera" | "mic" | "win_key" | "fn_lock" | "keep_awake") {
                return Err(format!("unknown switch '{}'", name));
            }
            let on = arg_bool(args, "on")?;
            let res = switches_cmd::set_device_switch(app.clone(), name.to_string(), on)?;
            changed(json!({ "switch": name, "on": res }))
        }
        "switches" => Ok(to_value(switches_cmd::get_device_switches())),
        "screen_off" => {
            display_cmd::turn_off_screen()?;
            Ok(json!({ "screen_off": true }))
        }
        "screen_on" => {
            display_cmd::turn_on_screen();
            Ok(json!({ "screen_off": false }))
        }
        "light" => {
            let lighting = app.state::<crate::driver::ite_lighting::SharedIteLightingManager>();
            let st = lighting.state.lock().clone();
            let (enabled, level) = match args.get("level").and_then(|v| v.as_u64()) {
                Some(0) => (false, 0u8),
                Some(n) => (true, n.min(255) as u8),
                None => {
                    let on = arg_bool(args, "on")?;
                    (on, if on { st.kb_brightness.max(1) } else { 0 })
                }
            };
            lighting_cmd::apply_keyboard_lighting(
                app.clone(), enabled, st.kb_effect, level, st.kb_speed, st.kb_direction, st.kb_color.clone(),
                None, None, None, None, None, None, lighting,
            )?;
            changed(json!({ "keyboard_light": enabled, "level": level }))
        }
        "ec_read" => {
            let addr = args.get("addr").and_then(|v| v.as_u64()).ok_or("missing argument 'addr'")?;
            let len = args.get("len").and_then(|v| v.as_u64()).unwrap_or(1).clamp(1, 256);
            if addr + len > 0x10000 {
                return Err("address range beyond 0xFFFF".into());
            }
            let addrs: Vec<u16> = (0..len).map(|i| (addr + i) as u16).collect();
            let bytes = crate::driver::wmi_ec::wmi_batch_read_ec(&addrs).ok_or("EC read failed")?;
            Ok(json!({ "addr": addr, "bytes": bytes }))
        }
        _ => Err(format!("unknown command '{}'", cmd)),
    }
}

mod catalog {
    include!(concat!(env!("OUT_DIR"), "/command_catalog.rs"));
}

/// Run a registered IPC command exactly as the panel's `invoke()` would: same argument
/// deserialisation (camelCase keys), same ACL check, same implementation, on the main thread.
/// Works even if the WebView2 process is gone, since dispatch happens on the Rust side.
fn invoke_like_frontend(app: &AppHandle, command: &str, payload: Value) -> Result<Value, String> {
    use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponse, InvokeResponseBody};
    let window = app.get_webview_window("main").ok_or("main window not available")?;
    let url = window.url().map_err(|e| format!("main window url: {}", e))?;
    let request = tauri::webview::InvokeRequest {
        cmd: command.to_string(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url,
        body: InvokeBody::Json(payload),
        headers: Default::default(),
        invoke_key: app.invoke_key().to_string(),
    };
    let (tx, rx) = std::sync::mpsc::channel::<Result<Value, String>>();
    app.run_on_main_thread(move || {
        window.on_message(
            request,
            Box::new(move |_webview, _cmd, response, _cb, _err| {
                let out = match response {
                    InvokeResponse::Ok(InvokeResponseBody::Json(s)) => Ok(serde_json::from_str(&s).unwrap_or(Value::String(s))),
                    InvokeResponse::Ok(InvokeResponseBody::Raw(b)) => Ok(json!({ "raw_bytes": b.len() })),
                    InvokeResponse::Err(e) => Err(match e.0 {
                        Value::String(s) => s,
                        other => other.to_string(),
                    }),
                };
                let _ = tx.send(out);
            }),
        );
    })
    .map_err(|e| format!("main thread dispatch failed: {}", e))?;
    rx.recv_timeout(std::time::Duration::from_secs(60))
        .map_err(|_| format!("'{}' did not answer within 60 s", command))?
}

fn handle(app: &AppHandle, line: &str) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": format!("bad request: {}", e) }),
    };
    let cmd = req.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
    let args = req.get("args").cloned().unwrap_or(Value::Null);
    let read_only_invoke = cmd == "invoke"
        && args.get("command").and_then(|v| v.as_str()).map_or(false, |c| c.starts_with("get_") || c.starts_with("read_") || c.starts_with("is_"));
    if !matches!(cmd, "ping" | "status" | "fan" | "profiles" | "switches" | "ec_read" | "catalog")
        && !(cmd == "gpu" && args.get("mode").is_none())
        && !read_only_invoke
    {
        crate::log_info!("CLI", "orv {} {}", cmd, args);
    }
    match dispatch(app, cmd, &args) {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => {
            crate::log_warn!("CLI", "orv {} failed: {}", cmd, e);
            json!({ "ok": false, "error": e })
        }
    }
}

#[cfg(windows)]
fn serve(app: AppHandle) {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_WAIT,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // DACL: SYSTEM, Administrators and the user this process runs as (the logged-in user; elevated via the task).
    let sddl = unsafe {
        let mut token = Default::default();
        let mut user_sid = String::new();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_ok() {
            let mut buf = vec![0u8; 256];
            let mut len = 0u32;
            if GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), buf.len() as u32, &mut len).is_ok() {
                let tu = &*(buf.as_ptr() as *const TOKEN_USER);
                let mut s = windows::core::PWSTR::null();
                if ConvertSidToStringSidW(tu.User.Sid, &mut s).is_ok() {
                    user_sid = s.to_string().unwrap_or_default();
                    let _ = LocalFree(HLOCAL(s.0 as _));
                }
            }
            let _ = CloseHandle(token);
        }
        let mut d = String::from("D:P(A;;GA;;;SY)(A;;GA;;;BA)");
        if !user_sid.is_empty() {
            d.push_str(&format!("(A;;GA;;;{})", user_sid));
        }
        d
    };
    let mut psd = PSECURITY_DESCRIPTOR::default();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl.as_str()), SDDL_REVISION_1, &mut psd, None) }
        .is_err()
    {
        crate::log_error!("CLI", "Could not build pipe security descriptor; orv disabled");
        return;
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: psd.0,
        bInheritHandle: false.into(),
    };
    let name = HSTRING::from(PIPE_NAME);
    crate::log_info!("CLI", "Command-line pipe ready at {} ({})", PIPE_NAME, sddl);

    loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                Some(&sa),
            )
        };
        if pipe.is_invalid() {
            crate::log_error!("CLI", "CreateNamedPipe failed; retrying in 10 s");
            std::thread::sleep(std::time::Duration::from_secs(10));
            continue;
        }
        let connected = match unsafe { ConnectNamedPipe(pipe, None) } {
            Ok(()) => true,
            Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
        };
        if connected {
            // Read one request line (bounded).
            let mut req = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let mut n = 0u32;
                if unsafe { ReadFile(pipe, Some(&mut chunk), Some(&mut n), None) }.is_err() || n == 0 {
                    break;
                }
                req.extend_from_slice(&chunk[..n as usize]);
                if req.contains(&b'\n') || req.len() > 64 * 1024 {
                    break;
                }
            }
            let line = String::from_utf8_lossy(&req);
            let resp = handle(&app, line.lines().next().unwrap_or(""));
            let mut out = resp.to_string().into_bytes();
            out.push(b'\n');
            let mut written = 0u32;
            unsafe {
                let _ = WriteFile(pipe, Some(&out), Some(&mut written), None);
                let _ = FlushFileBuffers(pipe);
                let _ = DisconnectNamedPipe(pipe);
            }
        }
        unsafe {
            let _ = CloseHandle(pipe);
        }
    }
}

#[cfg(not(windows))]
fn serve(_app: AppHandle) {}
