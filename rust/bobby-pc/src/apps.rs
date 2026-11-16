//! Launchable games/apps index, open windows, close, GPU status, and the `run` dispatcher for PC caps.
//! No PowerShell and no simulated input (Riot Vanguard). Side effects all go through `sh` / `record`.
use bobby_proto::msg::Cap;
use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::{w, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, TRUE};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, IBindCtx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Shell::{IEnumShellItems, IShellItem, SHGetKnownFolderItem, BHID_EnumItems, FOLDERID_AppsFolder, KF_FLAG_DEFAULT, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER};

// ---------- Dry mode and the one place that spawns ----------

static DRY: AtomicBool = AtomicBool::new(false);
pub static CALLS: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

/// Tests flip this; `BOBBY_PC_DRY=1` does the same from the environment.
pub fn set_dry(on: bool) {
    DRY.store(on, Ordering::SeqCst);
}
pub fn dry() -> bool {
    DRY.load(Ordering::SeqCst) || std::env::var("BOBBY_PC_DRY").as_deref() == Ok("1")
}
pub fn record(call: &[&str]) {
    CALLS.lock().unwrap().push(call.iter().map(|s| s.to_string()).collect());
}

/// Every spawned process goes through here. Hidden, detached, no input.
pub fn sh(file: &str, args: &[&str]) {
    let mut call = vec![file];
    call.extend_from_slice(args);
    record(&call);
    if dry() {
        return;
    }
    // DETACHED_PROCESS | CREATE_NO_WINDOW
    let _ = Command::new(file).args(args).creation_flags(0x0800_0008).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

// ---------- Fuzzy matching (port of sim/lev/norm from bot/util.js) ----------

pub fn norm(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let c = match c {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'a',
                'ç' | 'Ç' => 'c',
                'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
                'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
                'ñ' | 'Ñ' => 'n',
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'o',
                'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
                'ý' | 'ÿ' | 'Ý' => 'y',
                c => c,
            };
            c.to_lowercase().collect::<Vec<_>>()
        })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

pub fn lev(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1])));
        }
        prev = cur;
    }
    prev[b.len()]
}

pub fn sim(q: &str, c: &str) -> f64 {
    let (a, b) = (norm(q), norm(c));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    if a.len().min(b.len()) >= 3 && (a.contains(&b) || b.contains(&a)) {
        return 0.9;
    }
    let (x, y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    1.0 - lev(&x, &y) as f64 / x.len().max(y.len()) as f64
}

// ---------- Index of launchable games and apps (built once) ----------

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub name: String,
    /// "riot", "steam", "epic", "app" or "store".
    pub kind: String,
    /// Riot: the client exe. Others: what `explorer.exe` opens (url, .lnk path, shell:AppsFolder\<AUMID>).
    pub target: String,
}

fn app(name: &str, kind: &str, target: &str) -> App {
    App { name: name.into(), kind: kind.into(), target: target.into() }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten().flat_map(|e| if e.path().is_dir() { walk(&e.path()) } else { vec![e.path()] }).collect()
}
fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}
/// Values of every `"key"  "value"` line of a Valve KeyValues text.
fn vdf_all<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    text.lines()
        .filter_map(|l| {
            let mut q = l.split('"').skip(1).step_by(2);
            (q.next() == Some(key)).then(|| q.next()).flatten()
        })
        .collect()
}

fn riot() -> Vec<App> {
    let mut exe = r"C:\Riot Games\Riot Client\RiotClientServices.exe".to_string();
    if let Ok(j) = serde_json::from_str::<Value>(&read(Path::new(r"C:\ProgramData\Riot Games\RiotClientInstalls.json"))) {
        if let Some(p) = j["rc_live"].as_str().or(j["rc_default"].as_str()).filter(|s| !s.is_empty()) {
            exe = p.replace('/', "\\");
        }
    }
    if Path::new(&exe).exists() {
        vec![app("Valorant", "riot", &exe)]
    } else {
        vec![]
    }
}

fn steam() -> Vec<App> {
    let root = PathBuf::from(std::env::var("STEAM_DIR").unwrap_or_else(|_| r"C:\Program Files (x86)\Steam".into()));
    let mut libs = vec![root.clone()];
    libs.extend(vdf_all(&read(&root.join("steamapps/libraryfolders.vdf")), "path").into_iter().map(|p| PathBuf::from(p.replace("\\\\", "\\"))));
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for l in libs.into_iter().filter(|l| seen.insert(l.clone())) {
        let Ok(rd) = std::fs::read_dir(l.join("steamapps")) else { continue };
        for f in rd.flatten().map(|e| e.path()) {
            let fname = f.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !(fname.starts_with("appmanifest_") && fname.ends_with(".acf")) {
                continue;
            }
            let t = read(&f);
            let (name, id) = (vdf_all(&t, "name").first().copied(), vdf_all(&t, "appid").first().copied());
            let (Some(name), Some(id)) = (name, id) else { continue };
            let low = name.to_lowercase();
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) || ["steamworks", "proton", "steam linux"].iter().any(|x| low.contains(x)) {
                continue;
            }
            out.push(app(name, "steam", &format!("steam://rungameid/{id}")));
        }
    }
    out
}

fn epic() -> Vec<App> {
    walk(Path::new(r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests"))
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "item"))
        .filter_map(|f| {
            let j: Value = serde_json::from_str(&read(f)).ok()?;
            let (n, id) = (j["DisplayName"].as_str().filter(|s| !s.is_empty())?, j["AppName"].as_str().filter(|s| !s.is_empty())?);
            let enc: String = id.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
            Some(app(n, "epic", &format!("com.epicgames.launcher://apps/{enc}?action=launch&silent=true")))
        })
        .collect()
}

/// Same exclusion as the regex /uninstall|désinstall|readme|documentation|\bhelp\b/i.
fn excluded(base: &str) -> bool {
    let l = base.to_lowercase();
    if ["uninstall", "désinstall", "readme", "documentation"].iter().any(|x| l.contains(x)) {
        return true;
    }
    l.match_indices("help").any(|(i, _)| {
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !word(l[..i].chars().last()) && !word(l[i + 4..].chars().next())
    })
}

fn start_menu() -> Vec<App> {
    ["ProgramData", "APPDATA"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .flat_map(|d| walk(&Path::new(&d).join(r"Microsoft\Windows\Start Menu\Programs")))
        .filter_map(|f| {
            let ext = f.extension()?.to_str()?.to_lowercase();
            let stem = f.file_stem()?.to_str()?.to_string();
            let base = f.file_name()?.to_str()?;
            ((ext == "lnk" || ext == "url") && !excluded(base)).then(|| app(&stem, "app", f.to_str().unwrap()))
        })
        .collect()
}

/// Store apps: the shell AppsFolder lists them with their AppUserModelID; explorer opens them by it.
fn store() -> Vec<App> {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut out = vec![];
        let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else { return out };
        let Ok(en) = folder.BindToHandler::<_, IEnumShellItems>(None::<&IBindCtx>, &BHID_EnumItems) else { return out };
        let name_of = |it: &IShellItem, k| {
            it.GetDisplayName(k).ok().map(|p: PWSTR| {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            })
        };
        loop {
            let mut a = [None];
            let mut n = 0u32;
            let _ = en.Next(&mut a, Some(&mut n));
            let Some(it) = a[0].take().filter(|_| n > 0) else { break };
            if let (Some(name), Some(id)) = (name_of(&it, SIGDN_NORMALDISPLAY), name_of(&it, SIGDN_PARENTRELATIVEPARSING)) {
                if !name.is_empty() && id.contains('!') {
                    out.push(app(&name, "store", &format!("shell:AppsFolder\\{id}")));
                }
            }
        }
        out
    })
    .join()
    .unwrap_or_default()
}

/// Built once per run, in pc.js order; restart after installing a game.
pub fn index() -> &'static [App] {
    static INDEX: OnceLock<Vec<App>> = OnceLock::new();
    INDEX.get_or_init(|| [riot(), steam(), epic(), start_menu(), store()].concat())
}

/// Lowercase and strip leading articles / "jeu", "game", "app" words (twice, like pc.js ART).
fn strip_art(q: &str) -> String {
    let mut s = q.to_lowercase();
    for _ in 0..2 {
        let mut t = s.as_str();
        while let Some(r) = ["le ", "la ", "les ", "l'", "the ", "un ", "une "].iter().find_map(|p| t.strip_prefix(p)) {
            t = r;
        }
        if t.len() == s.len() {
            if let Some(r) = ["jeu", "game", "application", "app"].iter().find_map(|p| t.strip_prefix(p).and_then(|r| r.strip_prefix(char::is_whitespace))) {
                t = r.trim_start();
            }
        }
        s = t.to_string();
    }
    s
}

/// Best entry for a spoken name: best score, then shorter name, then index order (games first).
pub fn find_in<'a>(index: &'a [App], q: &str) -> Option<&'a App> {
    let name = strip_art(q);
    let mut best: Option<(&App, f64)> = None;
    for e in index {
        let s = sim(&name, &e.name);
        if s >= 0.75 && best.is_none_or(|(b, bs)| s > bs || (s == bs && e.name.chars().count() < b.name.chars().count())) {
            best = Some((e, s));
        }
    }
    best.map(|b| b.0)
}
pub fn find(q: &str) -> Option<&'static App> {
    find_in(index(), q)
}

pub fn launch(a: &App) {
    if a.kind == "riot" {
        sh(&a.target, &["--launch-product=valorant", "--launch-patchline=live"]);
    } else {
        sh("explorer.exe", &[&a.target]);
    }
}
pub fn close(pid: u32) {
    sh("taskkill.exe", &["/PID", &pid.to_string(), "/T", "/F"]);
}

// ---------- Open windows ----------

#[derive(Debug, Clone)]
pub struct Win {
    pub pid: u32,
    pub name: String,
    pub title: String,
    pub product: String,
}

const SELF: [&str; 14] = ["node", "python", "pythonw", "llama-server", "explorer", "powershell", "pwsh", "cmd", "conhost", "windowsterminal", "dwm", "svchost", "csrss", "winlogon"];

fn product_of(exe: &str) -> String {
    unsafe {
        let wide: Vec<u16> = exe.encode_utf16().chain([0]).collect();
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return String::new();
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(PCWSTR(wide.as_ptr()), None, size, buf.as_mut_ptr().cast()).is_err() {
            return String::new();
        }
        let (mut p, mut n) = (std::ptr::null_mut(), 0u32);
        if !VerQueryValueW(buf.as_ptr().cast(), w!("\\VarFileInfo\\Translation"), &mut p, &mut n).as_bool() || n < 4 {
            return String::new();
        }
        let [l0, l1, c0, c1] = *(p as *const [u8; 4]);
        let sub: Vec<u16> = format!("\\StringFileInfo\\{:04x}{:04x}\\ProductName", u16::from_le_bytes([l0, l1]), u16::from_le_bytes([c0, c1])).encode_utf16().chain([0]).collect();
        if !VerQueryValueW(buf.as_ptr().cast(), PCWSTR(sub.as_ptr()), &mut p, &mut n).as_bool() || n == 0 {
            return String::new();
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p as *const u16, n as usize)).split('\0').next().unwrap_or_default().to_string()
    }
}

fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// Visible, titled, unowned top-level windows (approximates Process.MainWindowHandle), one per process.
pub fn windows() -> Vec<Win> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        (*(l.0 as *mut Vec<HWND>)).push(h);
        TRUE
    }
    let mut hs: Vec<HWND> = vec![];
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut hs as *mut _ as isize));
    }
    let mut out: Vec<Win> = vec![];
    for h in hs {
        unsafe {
            if !IsWindowVisible(h).as_bool() || GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
                continue;
            }
            let mut t = [0u16; 512];
            let n = GetWindowTextW(h, &mut t);
            if n <= 0 {
                continue;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == 0 || out.iter().any(|w| w.pid == pid) {
                continue;
            }
            let Some(exe) = exe_of(pid) else { continue };
            let name = Path::new(&exe).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if SELF.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                continue;
            }
            out.push(Win { pid, name, title: String::from_utf16_lossy(&t[..n as usize]), product: product_of(&exe) });
        }
    }
    out
}

/// Best open window for a spoken name ("Apex Legends" matches the title, "r5apex" the process).
pub fn pick_window<'a>(ws: &'a [Win], q: &str) -> Option<&'a Win> {
    let name = strip_art(q);
    let mut best: Option<(&Win, f64)> = None;
    for w in ws {
        let s = [&w.title, &w.name, &w.product].iter().filter(|c| !c.is_empty()).map(|c| sim(&name, c)).fold(0.0, f64::max);
        if s >= 0.75 && best.is_none_or(|(_, bs)| s > bs) {
            best = Some((w, s));
        }
    }
    best.map(|b| b.0)
}
pub fn open_window(q: &str) -> Option<Win> {
    pick_window(&windows(), q).cloned()
}

// ---------- GPU ----------

pub fn gpu() -> Result<Value, String> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=name,temperature.gpu,utilization.gpu,memory.used,memory.total", "--format=csv,noheader"])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("nvidia-smi failed".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let f: Vec<&str> = text.trim().split(',').map(str::trim).collect();
    if f.len() < 5 {
        return Err(format!("unexpected nvidia-smi output: {}", text.trim()));
    }
    let num = |v: &str| v.split_whitespace().next().and_then(|n| n.parse::<f64>().ok()).unwrap_or(0.0);
    let gb = |v: &str| format!("{:.1}", num(v) / 1024.0);
    Ok(json!({ "name": f[0].replace("NVIDIA GeForce ", ""), "temp": f[1], "load": num(f[2]) as i64, "used": gb(f[3]), "total": gb(f[4]) }))
}

// ---------- Dispatcher ----------

/// `None` for caps that are not apps/power/gpu.
pub fn run(cap: &Cap) -> Option<Result<Value, String>> {
    Some(match cap {
        Cap::PcApps => Ok(json!({ "items": index().iter().map(|a| a.name.clone()).collect::<Vec<_>>() })),
        Cap::PcWindows => Ok(json!({ "items": windows().iter().map(|w| format!("{} ({})", w.title, w.name)).collect::<Vec<_>>() })),
        Cap::PcLaunch { name } => Ok(json!({ "found": find(name).map(|a| { launch(a); a.name.clone() }) })),
        Cap::PcClose { name } if dry() => {
            sh("taskkill.exe", &["/IM", name, "/T", "/F"]);
            Ok(json!({ "found": name }))
        }
        Cap::PcClose { name } => Ok(json!({ "found": open_window(name).map(|w| { close(w.pid); if w.title.is_empty() { w.name } else { w.title } }) })),
        Cap::PcLock => {
            crate::power::lock();
            Ok(json!({}))
        }
        Cap::PcSleep => {
            crate::power::sleep();
            Ok(json!({}))
        }
        Cap::PcShutdown => {
            crate::power::shutdown();
            Ok(json!({}))
        }
        Cap::PcGpu => gpu(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static LOCK: Mutex<()> = Mutex::new(());
    /// Dry mode on, recorded calls cleared; the guard serializes tests that read `CALLS`.
    fn dry_guard() -> std::sync::MutexGuard<'static, ()> {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_dry(true);
        CALLS.lock().unwrap().clear();
        g
    }
    fn calls() -> Vec<Vec<String>> {
        CALLS.lock().unwrap().clone()
    }

    #[test]
    fn sim_cases() {
        assert_eq!(sim("🎮 Gaming", "gaming"), 1.0);
        assert_eq!(norm("Éléphant Ça!"), "elephantca");
        assert_eq!(sim("apex", "Apex Legends"), 0.9);
        assert_eq!(sim("", "x"), 0.0);
        assert!(sim("valorent", "valorant") >= 0.75);
        assert!(sim("chrome", "steam") < 0.5);
        let (a, b): (Vec<char>, Vec<char>) = ("kitten".chars().collect(), "sitting".chars().collect());
        assert_eq!(lev(&a, &b), 3);
    }

    #[test]
    fn find_on_injected_index() {
        let idx = vec![app("Valorant", "riot", "x"), app("Apex Legends", "steam", "a"), app("Apex Legends Launcher Tool", "app", "b"), app("Claude", "store", "c"), app("Claude", "app", "d")];
        assert_eq!(find_in(&idx, "le jeu valorant").unwrap().name, "Valorant");
        assert_eq!(find_in(&idx, "apex").unwrap().kind, "steam");
        assert_eq!(find_in(&idx, "Claude").unwrap().kind, "store"); // tie: first in index order
        assert!(find_in(&idx, "zzzzzz").is_none());
        assert!(excluded("Uninstall Foo.lnk") && excluded("Foo Help.lnk") && !excluded("Helpful.lnk"));
    }

    #[test]
    fn run_dry_records_calls() {
        let _g = dry_guard();
        assert_eq!(run(&Cap::PcClose { name: "Apex".into() }).unwrap().unwrap(), json!({ "found": "Apex" }));
        run(&Cap::PcLock).unwrap().unwrap();
        run(&Cap::PcSleep).unwrap().unwrap();
        run(&Cap::PcShutdown).unwrap().unwrap();
        launch(&app("Valorant", "riot", r"C:\rc.exe"));
        launch(&app("Steamy", "steam", "steam://rungameid/1"));
        close(42);
        let want: Vec<Vec<&str>> = vec![
            vec!["taskkill.exe", "/IM", "Apex", "/T", "/F"],
            vec!["LockWorkStation"],
            vec!["SetSuspendState", "false", "false", "false"],
            vec!["shutdown.exe", "/s", "/t", "5"],
            vec![r"C:\rc.exe", "--launch-product=valorant", "--launch-patchline=live"],
            vec!["explorer.exe", "steam://rungameid/1"],
            vec!["taskkill.exe", "/PID", "42", "/T", "/F"],
        ];
        assert_eq!(calls(), want);
        assert!(run(&Cap::PcNow).is_none());
        assert_eq!(run(&Cap::PcLaunch { name: "zzzzqqqq nothing".into() }).unwrap().unwrap(), json!({ "found": null }));
        assert_eq!(calls().len(), want.len());
    }

    #[test]
    fn real_read_only() {
        let _g = dry_guard();
        let ws = windows();
        println!("windows: {:?}", ws.iter().map(|w| (&w.name, &w.product)).collect::<Vec<_>>());
        assert!(!ws.is_empty());
        let idx = index();
        for k in ["riot", "steam", "epic", "app", "store"] {
            println!("{k}: {}", idx.iter().filter(|a| a.kind == k).count());
        }
        assert!(idx.iter().any(|a| a.kind == "store"));
        let items = run(&Cap::PcApps).unwrap().unwrap();
        assert_eq!(items["items"].as_array().unwrap().len(), idx.len());
        println!("gpu: {:?}", gpu());
        assert!(calls().is_empty());
    }
}
