// orv: command-line remote for the running OpenRevo (talks to \\.\pipe\openrevo-cli; no admin needed).
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom, Write};

const PIPE: &str = r"\\.\pipe\openrevo-cli";

const HELP: &str = "orv - OpenRevo 命令行

  orv status [--json] [--gpu]       状态一览（混合模式下默认不唤醒独显，--gpu 强制读取）
  orv watch [秒] [--gpu]            持续刷新状态（默认 2 秒）
  orv mode office|balanced|turbo|custom [配置名]
  orv boost on|off                  一键强冷
  orv fan [--json]                  EC 风扇表与是否已应用
  orv fan policy tuned|official     预设模式风扇曲线：调校 / 原厂
  orv profile list                  自定义配置列表
  orv profile apply <名称或 ID>     应用自定义配置（= mode custom <名称>）
  orv gpu [hybrid|dgpu|igpu --yes [--reboot]]   显卡模式（需重启）
  orv hz <刷新率>                   如 orv hz 60 / orv hz 300
  orv awake on|off                  防休眠
  orv screen off|on                 黑屏不待机（系统继续运行；按任意键恢复，鼠标不会唤醒）
  orv osd on|off                    游戏内 RTSS OSD
  orv light on|off|<亮度>           键盘灯
  orv lock win|fn on|off            Win 键锁 / Fn 锁
  orv wifi|bluetooth|touchpad|camera|mic on|off
  orv switches [--json]             各开关当前状态
  orv ec read <地址> [长度]         EC 只读（地址可写 0x...）
  orv log [行数]                    最近的日志（默认 30 行）

完整功能（与界面对齐，界面能做的都能调）：
  orv commands [关键词]             列出后端全部命令及参数（按模块分组）
  orv call <命令> [参数=值 ...]     调用任意命令，例：orv call set_display_brightness brightness=60
  orv call <命令> '<JSON>'          参数也可直接给 JSON 对象
    参数名写 snake_case 或 camelCase 都行；值按 JSON 解析（数字、true/false、[..]、{..}），否则当字符串
";

fn die(msg: &str, code: i32) -> ! {
    eprintln!("orv: {}", msg);
    std::process::exit(code)
}

fn call(cmd: &str, args: Value) -> Value {
    let mut tries = 0;
    let mut pipe = loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(PIPE) {
            Ok(f) => break f,
            Err(e) => match e.raw_os_error() {
                Some(231) if tries < 40 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Some(2) => die("OpenRevo 没在运行（或版本太旧，不带命令行接口）", 3),
                Some(5) => die("没有权限连接 OpenRevo（管道只对当前用户开放）", 3),
                _ => die(&format!("连接 OpenRevo 失败: {}", e), 3),
            },
        }
    };
    let mut req = json!({ "cmd": cmd, "args": args }).to_string();
    req.push('\n');
    if let Err(e) = pipe.write_all(req.as_bytes()) {
        die(&format!("发送失败: {}", e), 3);
    }
    let mut buf = String::new();
    let _ = pipe.read_to_string(&mut buf);
    let resp: Value = serde_json::from_str(buf.trim()).unwrap_or_else(|_| die("OpenRevo 返回了无法解析的结果", 3));
    if resp["ok"].as_bool() != Some(true) {
        die(resp["error"].as_str().unwrap_or("未知错误"), 1);
    }
    resp["data"].clone()
}

fn on_off(s: Option<&String>) -> bool {
    match s.map(|x| x.as_str()) {
        Some("on") | Some("1") | Some("true") | Some("开") => true,
        Some("off") | Some("0") | Some("false") | Some("关") => false,
        _ => die("需要 on 或 off", 2),
    }
}

fn yn(b: &Value) -> &'static str {
    if b.as_bool().unwrap_or(false) {
        "开"
    } else {
        "关"
    }
}

fn mode_cn(m: &str) -> &'static str {
    match m {
        "office" => "办公",
        "balanced" => "均衡",
        "turbo" => "狂暴",
        "custom" => "自定义",
        _ => "未知",
    }
}

fn fan_state_cn(s: &str) -> &'static str {
    match s {
        "applied" => "已应用",
        "mismatch" => "与预期不一致",
        "fan_boost" => "一键强冷中",
        _ => "无法读取",
    }
}

fn print_status(s: &Value) {
    let mut mode = format!("{} ({})", mode_cn(s["mode"].as_str().unwrap_or("")), s["mode"].as_str().unwrap_or(""));
    if s["mode"] == "custom" && !s["custom_profile"].as_str().unwrap_or("").is_empty() {
        mode.push_str(&format!(" · {}", s["custom_profile"].as_str().unwrap()));
    }
    println!(
        "模式   {}   强冷 {}   风扇表 {} · {}",
        mode,
        yn(&s["fan_boost"]),
        s["fan_policy"].as_str().unwrap_or(""),
        fan_state_cn(s["fan_table"].as_str().unwrap_or(""))
    );
    let c = &s["cpu"];
    println!("CPU    {}°C  {}W  {}MHz  {}%", c["temp_c"], c["power_w"], c["freq_mhz"], c["usage_pct"]);
    let g = &s["gpu"];
    if g.is_null() {
        println!("GPU    （{} 模式，独显未查询；加 --gpu 读取会唤醒独显）", s["gpu_mode"].as_str().unwrap_or("?"));
    } else {
        let thr = g["throttle"].as_str().unwrap_or("");
        println!(
            "GPU    {}°C  {}/{}W  {}MHz  {}%  显存 {:.1}/{:.0}G{}",
            g["temp_c"],
            g["power_w"],
            g["limit_w"],
            g["core_mhz"],
            g["usage_pct"],
            g["vram_used_mb"].as_f64().unwrap_or(0.0) / 1024.0,
            g["vram_total_mb"].as_f64().unwrap_or(0.0) / 1024.0,
            if thr.is_empty() { String::new() } else { format!("  限制:{}", thr) }
        );
    }
    println!("风扇   CPU {} / GPU {} rpm", s["fans_rpm"]["cpu"], s["fans_rpm"]["gpu"]);
    println!("内存   已用 {}G", s["ram_used_gb"]);
    let b = &s["battery"];
    let power = if b["charging"].as_bool().unwrap_or(false) {
        "充电中"
    } else if b["ac"].as_bool().unwrap_or(false) {
        "已接电源"
    } else {
        "电池供电"
    };
    println!("电池   {}%  {}", b["percent"], power);
    println!(
        "其他   {} Hz   防休眠 {}   OSD {}   Win 锁 {}{}",
        s["refresh_hz"],
        yn(&s["keep_awake"]),
        yn(&s["rtss_osd"]),
        yn(&s["win_key_locked"]),
        if s["screen_off"].as_bool().unwrap_or(false) { "   屏幕已关闭" } else { "" }
    );
}

/// Left-align counting CJK characters as two columns.
fn pad(label: &str, width: usize) -> String {
    let w: usize = label.chars().map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 }).sum();
    format!("{}{}", label, " ".repeat(width.saturating_sub(w)))
}

fn camel(s: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in s.chars() {
        if c == '_' {
            up = !out.is_empty();
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn signature(c: &Value) -> String {
    c["params"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|p| format!("{}{}: {}", p["name"].as_str().unwrap_or(""), if p["optional"].as_bool().unwrap_or(false) { "?" } else { "" }, p["type"].as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn list_commands(filter: Option<&str>, json_out: bool) {
    let cat = call("catalog", json!({}));
    let mut items: Vec<Value> = cat.as_array().cloned().unwrap_or_default();
    if let Some(f) = filter {
        let f = f.to_lowercase();
        items.retain(|c| c["name"].as_str().unwrap_or("").contains(&f) || c["module"].as_str().unwrap_or("").to_lowercase().contains(&f));
    }
    if json_out {
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
        return;
    }
    items.sort_by(|a, b| (a["module"].as_str(), a["name"].as_str()).cmp(&(b["module"].as_str(), b["name"].as_str())));
    let mut last = String::new();
    for c in &items {
        let m = c["module"].as_str().unwrap_or("").to_string();
        if m != last {
            println!("\n[{}]", m);
            last = m;
        }
        println!("  {}({})", c["name"].as_str().unwrap_or(""), signature(c));
    }
    println!("\n共 {} 个命令。用法：orv call <命令> 参数=值 ...", items.len());
}

fn generic_call(pos: &[&String], json_out: bool) {
    let name = pos.get(1).map(|s| s.as_str()).unwrap_or_else(|| die("用法: orv call <命令> [参数=值 ...]", 2));
    let rest: Vec<&str> = pos[2..].iter().map(|s| s.as_str()).collect();
    let args = if rest.len() == 1 && rest[0].trim_start().starts_with('{') {
        serde_json::from_str(rest[0]).unwrap_or_else(|e| die(&format!("JSON 参数无效: {}", e), 2))
    } else {
        let mut m = serde_json::Map::new();
        for kv in rest {
            let (k, v) = kv.split_once('=').unwrap_or_else(|| die(&format!("参数要写成 名称=值: {}", kv), 2));
            let val = serde_json::from_str::<Value>(v).unwrap_or_else(|_| Value::String(v.to_string()));
            m.insert(camel(k), val);
        }
        Value::Object(m)
    };
    // Check names against the catalog so a typo gives the real signature instead of a vague error.
    let cat = call("catalog", json!({}));
    let entry = cat.as_array().and_then(|a| a.iter().find(|c| c["name"] == name).cloned());
    let Some(entry) = entry else {
        let close: Vec<String> = cat.as_array().cloned().unwrap_or_default().iter().filter_map(|c| c["name"].as_str().map(String::from)).filter(|n| n.contains(name) || name.contains(n.as_str())).take(8).collect();
        die(&format!("没有命令 '{}'{}", name, if close.is_empty() { String::new() } else { format!("，是不是: {}", close.join(", ")) }), 2)
    };
    let params = entry["params"].as_array().cloned().unwrap_or_default();
    for k in args.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
        if !params.iter().any(|p| p["name"] == k) {
            die(&format!("{} 没有参数 '{}'。签名: {}({})", name, k, name, signature(&entry)), 2);
        }
    }
    for p in &params {
        if !p["optional"].as_bool().unwrap_or(false) && args.get(p["name"].as_str().unwrap_or("")).is_none() {
            die(&format!("缺少参数 '{}'。签名: {}({})", p["name"].as_str().unwrap_or(""), name, signature(&entry)), 2);
        }
    }
    let r = call("invoke", json!({ "command": name, "args": args }));
    if json_out || !r.is_null() {
        println!("{}", if r.is_string() && !json_out { r.as_str().unwrap().to_string() } else { serde_json::to_string_pretty(&r).unwrap_or_default() });
    } else {
        println!("完成");
    }
}

fn parse_num(s: &str) -> u64 {
    let r = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16)
    } else {
        s.parse()
    };
    r.unwrap_or_else(|_| die(&format!("不是有效数字: {}", s), 2))
}

fn show_log(n: usize) {
    let path = std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join(r"OpenRevo\open-revo.log");
    let mut f = std::fs::File::open(&path).unwrap_or_else(|e| die(&format!("打不开 {}: {}", path.display(), e), 1));
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(512 * 1024);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    for l in &lines[lines.len().saturating_sub(n)..] {
        println!("{}", l);
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| argv.iter().any(|a| a == f);
    let pos: Vec<&String> = argv.iter().filter(|a| !a.starts_with("--")).collect();
    let p = |i: usize| pos.get(i).copied();
    let json_out = flag("--json");
    let print = |v: &Value| println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());

    match p(0).map(|s| s.as_str()) {
        None | Some("help") | Some("-h") | Some("/?") => print!("{}", HELP),
        Some("status") => {
            let s = call("status", json!({ "gpu": flag("--gpu") }));
            if json_out { print(&s) } else { print_status(&s) }
        }
        Some("watch") => {
            let secs = p(1).map(|s| parse_num(s)).unwrap_or(2).max(1);
            loop {
                let s = call("status", json!({ "gpu": flag("--gpu") }));
                print!("\x1b[2J\x1b[H");
                print_status(&s);
                println!("\n（每 {} 秒刷新，Ctrl+C 退出）", secs);
                let _ = std::io::stdout().flush();
                std::thread::sleep(std::time::Duration::from_secs(secs));
            }
        }
        Some("mode") => {
            let m = p(1).unwrap_or_else(|| die("用法: orv mode office|balanced|turbo|custom [配置名]", 2));
            let mut args = json!({ "mode": m });
            if pos.len() > 2 {
                args["profile"] = json!(pos[2..].iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "));
            }
            let r = call("mode", args);
            println!("已切换到 {}", mode_cn(r["mode"].as_str().unwrap_or("")));
        }
        Some("boost") => {
            let on = on_off(p(1));
            call("boost", json!({ "on": on }));
            println!("一键强冷 {}", if on { "开" } else { "关" });
        }
        Some("fan") => {
            if p(1).map(|s| s.as_str()) == Some("policy") {
                let pol = p(2).unwrap_or_else(|| die("用法: orv fan policy tuned|official", 2));
                call("fan_policy", json!({ "policy": pol }));
                println!("预设风扇曲线已切换为 {}，已重新写入 EC", pol);
            } else {
                let f = call("fan", json!({}));
                if json_out {
                    print(&f);
                } else {
                    let a = &f["apply"];
                    println!("风扇表   {} · {}（曲线 {}）", fan_state_cn(a["state"].as_str().unwrap_or("")), a["source"].as_str().unwrap_or(""), f["policy"].as_str().unwrap_or(""));
                    if let Some(ec) = a.get("ec").filter(|v| !v.is_null()) {
                        for (k, label) in [("cpu", "CPU"), ("gpu", "GPU")] {
                            let row = |name: &str| ec[format!("{}_{}", k, name)].as_array().cloned().unwrap_or_default().iter().map(|v| format!("{:>3}", v)).collect::<Vec<_>>().join(" ");
                            println!("{} 升温 °C {}", label, row("upt"));
                            println!("{} 降温 °C {}", label, row("downt"));
                            println!("{} 占空 %  {}", label, row("duty_pct"));
                        }
                    }
                }
            }
        }
        Some("profile") => match p(1).map(|s| s.as_str()) {
            Some("list") | None => {
                let list = call("profiles", json!({}));
                if json_out {
                    print(&list);
                } else {
                    for x in list.as_array().cloned().unwrap_or_default() {
                        println!("{} {}  ({}){}", if x["active"].as_bool().unwrap_or(false) { "*" } else { " " }, x["name"].as_str().unwrap_or(""), x["id"].as_str().unwrap_or(""), if x["builtin"].as_bool().unwrap_or(false) { "  内置" } else { "" });
                    }
                }
            }
            Some("apply") => {
                if pos.len() < 3 {
                    die("用法: orv profile apply <名称或 ID>", 2);
                }
                let name = pos[2..].iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
                call("mode", json!({ "mode": "custom", "profile": name }));
                println!("已应用自定义配置: {}", name);
            }
            _ => die("用法: orv profile list | apply <名称>", 2),
        },
        Some("gpu") => {
            let mut args = json!({});
            if let Some(m) = p(1) {
                args = json!({ "mode": m, "yes": flag("--yes"), "reboot": flag("--reboot") });
            }
            let r = call("gpu", args);
            if json_out || p(1).is_none() {
                if json_out { print(&r) } else { println!("显卡模式: {}  ({})", r["current_mode"].as_str().unwrap_or("?"), r["gpu_name"].as_str().unwrap_or("")) }
            } else {
                println!("显卡模式已设为 {}，{}", p(1).unwrap(), if flag("--reboot") { "正在重启…" } else { "重启后生效" });
            }
        }
        Some("hz") => {
            let hz = parse_num(p(1).unwrap_or_else(|| die("用法: orv hz <刷新率>", 2)));
            call("hz", json!({ "hz": hz }));
            println!("刷新率 {} Hz", hz);
        }
        Some("awake") => {
            let on = on_off(p(1));
            call("switch", json!({ "name": "keep_awake", "on": on }));
            println!("防休眠 {}", if on { "开" } else { "关" });
        }
        Some("screen") => {
            match p(1).map(|s| s.as_str()) {
                Some("off") => {
                    call("screen_off", json!({}));
                    println!("屏幕已关闭，系统继续运行（按任意键恢复，鼠标不会唤醒）");
                }
                Some("on") => {
                    call("screen_on", json!({}));
                    println!("屏幕已恢复");
                }
                _ => die("用法: orv screen off|on", 2),
            }
        }
        Some("osd") => {
            let on = on_off(p(1));
            call("osd", json!({ "on": on }));
            println!("游戏内 OSD {}", if on { "开" } else { "关" });
        }
        Some("light") => {
            let a = p(1).unwrap_or_else(|| die("用法: orv light on|off|<亮度>", 2));
            let args = match a.as_str() {
                "on" | "off" => json!({ "on": a == "on" }),
                n => json!({ "level": parse_num(n) }),
            };
            let r = call("light", args);
            println!("键盘灯 {}{}", if r["keyboard_light"].as_bool().unwrap_or(false) { "开" } else { "关" }, if r["level"].as_u64().unwrap_or(0) > 0 { format!("，亮度 {}", r["level"]) } else { String::new() });
        }
        Some("lock") => {
            let name = match p(1).map(|s| s.as_str()) {
                Some("win") => "win_key",
                Some("fn") => "fn_lock",
                _ => die("用法: orv lock win|fn on|off", 2),
            };
            let on = on_off(p(2));
            call("switch", json!({ "name": name, "on": on }));
            println!("{} {}", if name == "win_key" { "Win 键锁" } else { "Fn 锁" }, if on { "开" } else { "关" });
        }
        Some(sw @ ("wifi" | "bluetooth" | "touchpad" | "camera" | "mic")) => {
            let on = on_off(p(1));
            let r = call("switch", json!({ "name": sw, "on": on }));
            println!("{} {}", sw, yn(&r["on"]));
        }
        Some("switches") => {
            let s = call("switches", json!({}));
            if json_out {
                print(&s);
            } else {
                for (k, label) in [("wifi", "Wi-Fi"), ("bluetooth", "蓝牙"), ("touchpad", "触控板"), ("camera", "摄像头"), ("mic", "麦克风"), ("win_key", "Win 键锁"), ("fn_lock", "Fn 锁"), ("keep_awake", "防休眠")] {
                    println!("{} {}", pad(label, 10), yn(&s[k]));
                }
            }
        }
        Some("ec") => {
            if p(1).map(|s| s.as_str()) != Some("read") {
                die("用法: orv ec read <地址> [长度]   （只读）", 2);
            }
            let addr = parse_num(p(2).unwrap_or_else(|| die("缺少地址", 2)));
            let len = p(3).map(|s| parse_num(s)).unwrap_or(1);
            let r = call("ec_read", json!({ "addr": addr, "len": len }));
            if json_out {
                print(&r);
            } else {
                let bytes: Vec<u64> = r["bytes"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_u64()).collect();
                for (i, row) in bytes.chunks(16).enumerate() {
                    println!("{:04X}: {}", addr as usize + i * 16, row.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" "));
                }
            }
        }
        Some("commands") => list_commands(p(1).map(|s| s.as_str()), json_out),
        Some("call") => generic_call(&pos, json_out),
        Some("log") => show_log(p(1).map(|s| parse_num(s) as usize).unwrap_or(30)),
        Some(other) => die(&format!("未知命令 '{}'，运行 orv help 查看用法", other), 2),
    }
}
