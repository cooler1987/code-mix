#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rfd::FileDialog;
use windows_sys::Win32::Foundation::{
    HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, GetDC, GetDeviceCaps, ReleaseDC, SetBkMode,
    SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    FF_DONTCARE, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, LOGPIXELSX, OUT_DEFAULT_PRECIS,
    TRANSPARENT,
};
use windows_sys::Win32::System::ApplicationInstallationAndServicing::{
    ActivateActCtx, CreateActCtxW, ACTCTXW,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
};
use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    EnableWindow, GetMessageW, GetSystemMetrics, GetWindowTextLengthW, GetWindowTextW,
    IsDialogMessageW, LoadCursorW, LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, SendMessageW, SetProcessDPIAware, SetWindowTextW, ShowWindow,
    TranslateMessage, UpdateWindow, BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX,
    BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW,
    EN_CHANGE, ES_AUTOHSCROLL, HICON, HMENU, ICON_BIG, ICON_SMALL, IDC_ARROW, MB_ICONERROR,
    MB_OK, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, WM_CLOSE, WM_COMMAND, WM_CREATE,
    WM_CTLCOLORBTN, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DROPFILES, WM_SETFONT, WM_SETICON, WM_USER,
    WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_ACCEPTFILES, WS_EX_CLIENTEDGE, WS_GROUP,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const ID_RADIO_URL: i32 = 101;
const ID_RADIO_DIR: i32 = 102;
const ID_RADIO_ZIP: i32 = 103;
const ID_RADIO_RESTORE: i32 = 104;
const ID_EDIT_PATH: i32 = 105;
const ID_BTN_BROWSE: i32 = 106;
const ID_RADIO_XML: i32 = 107;
const ID_RADIO_MD: i32 = 108;
const ID_RADIO_PLAIN: i32 = 109;
const ID_CHK_SUMMARY: i32 = 110;
const ID_CHK_STRUCTURE: i32 = 111;
const ID_CHK_LOG: i32 = 112;
const ID_BTN_PACK: i32 = 113;
const ID_BTN_RESET: i32 = 114;
const ID_LBL_STATUS: i32 = 115;

const WM_APP_DONE: u32 = WM_USER + 1;
const EM_SETCUEBANNER: u32 = 0x1501;
const BST_UNCHECKED: usize = 0;
const BST_CHECKED: usize = 1;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_SINGLE_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LOG_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputType {
    Url,
    Folder,
    Zip,
    Restore,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Xml,
    Markdown,
    Plain,
}

struct UiState {
    h_radio_url: HWND,
    h_radio_dir: HWND,
    h_radio_zip: HWND,
    h_radio_restore: HWND,
    h_edit_path: HWND,
    h_btn_browse: HWND,
    h_radio_xml: HWND,
    h_radio_md: HWND,
    h_radio_plain: HWND,
    h_chk_summary: HWND,
    h_chk_structure: HWND,
    h_chk_log: HWND,
    h_btn_pack: HWND,
    h_btn_reset: HWND,
    h_lbl_status: HWND,
    font_normal: HFONT,
    font_title: HFONT,
    bg_brush: HBRUSH,
    is_processing: bool,
    suppress_edit_change: bool,
}

thread_local! {
    static UI: RefCell<Option<UiState>> = const { RefCell::new(None) };
}

static WORK_RESULT: OnceLock<Mutex<Option<(bool, String)>>> = OnceLock::new();
static LOG_ENABLED: AtomicBool = AtomicBool::new(false);
static LOG_MUTEX: Mutex<()> = Mutex::new(());
static STARTUP_LOG_ARG: AtomicBool = AtomicBool::new(false);

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn scale(val: i32, dpi: i32) -> i32 {
    (val * dpi + 48) / 96
}

fn local_timestamp() -> String {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

fn current_date_string() -> String {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
}

fn log_file_path() -> PathBuf {
    let base = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("repomix-portable.log")
}

fn is_log_enabled() -> bool {
    LOG_ENABLED.load(Ordering::Relaxed)
}

fn write_log_line(level: &str, msg: &str) {
    if !is_log_enabled() {
        return;
    }
    let _guard = LOG_MUTEX.lock();
    let path = log_file_path();
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_LOG_FILE_BYTES {
            let _ = fs::write(&path, b"");
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] [{}] {}", local_timestamp(), level, msg);
    }
}

fn set_log_enabled(enable: bool) {
    LOG_ENABLED.store(enable, Ordering::Relaxed);
    if enable {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        write_log_line(
            "INFO",
            &format!(
                "日志功能已开启 | 版本 v{} | 架构 {} | 程序路径: {}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::ARCH,
                exe
            ),
        );
    }
}

unsafe fn show_error_box(hwnd: HWND, title: &str, msg: &str) {
    let t = to_wide(title);
    let m = to_wide(msg);
    MessageBoxW(hwnd, m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONERROR);
}

unsafe fn enable_modern_ui_and_dpi() {
    SetProcessDPIAware();

    let shell32 = to_wide("shell32.dll");
    let mut actx: ACTCTXW = std::mem::zeroed();
    actx.cbSize = std::mem::size_of::<ACTCTXW>() as u32;
    actx.dwFlags = 0x0000_0008; // ACTCTX_FLAG_RESOURCE_NAME_VALID
    actx.lpSource = shell32.as_ptr();
    actx.lpResourceName = 124usize as *const u16;

    let h_act = CreateActCtxW(&actx);
    if !h_act.is_null() && h_act != INVALID_HANDLE_VALUE {
        let mut cookie: usize = 0;
        ActivateActCtx(h_act, &mut cookie);
    }
}

unsafe fn set_text(hwnd: HWND, text: &str) {
    let wide = to_wide(text);
    SetWindowTextW(hwnd, wide.as_ptr());
}

unsafe fn set_edit_text_silently(ui: &mut UiState, text: &str) {
    ui.suppress_edit_change = true;
    set_text(ui.h_edit_path, text);
    ui.suppress_edit_change = false;
}

unsafe fn get_text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..copied as usize])
        .trim()
        .to_string()
}

unsafe fn is_checked(hwnd: HWND) -> bool {
    SendMessageW(hwnd, BM_GETCHECK, 0, 0) as usize == BST_CHECKED
}

unsafe fn set_checked(hwnd: HWND, checked: bool) {
    let state = if checked { BST_CHECKED } else { BST_UNCHECKED };
    SendMessageW(hwnd, BM_SETCHECK, state, 0);
}

unsafe fn set_cue(hwnd: HWND, cue: &str) {
    let wide = to_wide(cue);
    SendMessageW(hwnd, EM_SETCUEBANNER, 1, wide.as_ptr() as LPARAM);
}

unsafe fn get_input_type(ui: &UiState) -> InputType {
    if is_checked(ui.h_radio_url) {
        InputType::Url
    } else if is_checked(ui.h_radio_zip) {
        InputType::Zip
    } else if is_checked(ui.h_radio_restore) {
        InputType::Restore
    } else {
        InputType::Folder
    }
}

unsafe fn set_input_type_radio(ui: &UiState, input_type: InputType) {
    set_checked(ui.h_radio_url, input_type == InputType::Url);
    set_checked(ui.h_radio_dir, input_type == InputType::Folder);
    set_checked(ui.h_radio_zip, input_type == InputType::Zip);
    set_checked(ui.h_radio_restore, input_type == InputType::Restore);
}

unsafe fn get_output_format(ui: &UiState) -> OutputFormat {
    if is_checked(ui.h_radio_xml) {
        OutputFormat::Xml
    } else if is_checked(ui.h_radio_plain) {
        OutputFormat::Plain
    } else {
        OutputFormat::Markdown
    }
}

unsafe fn refresh_input_ui(ui: &UiState) {
    let mode = get_input_type(ui);
    let enable_pack_opts = if mode == InputType::Restore || ui.is_processing {
        0
    } else {
        1
    };

    EnableWindow(ui.h_radio_xml, enable_pack_opts);
    EnableWindow(ui.h_radio_md, enable_pack_opts);
    EnableWindow(ui.h_radio_plain, enable_pack_opts);
    EnableWindow(ui.h_chk_summary, enable_pack_opts);
    EnableWindow(ui.h_chk_structure, enable_pack_opts);

    match mode {
        InputType::Url => {
            EnableWindow(ui.h_btn_browse, 0);
            set_cue(
                ui.h_edit_path,
                "请输入 Git 仓库链接（支持 /tree/分支名，如 https://github.com/user/repo）",
            );
            set_text(ui.h_btn_pack, "开始打包 (Pack)");
        }
        InputType::Folder => {
            EnableWindow(ui.h_btn_browse, if ui.is_processing { 0 } else { 1 });
            set_cue(
                ui.h_edit_path,
                "点击右侧“浏览”或直接拖放本地项目文件夹到此窗口...",
            );
            set_text(ui.h_btn_pack, "开始打包 (Pack)");
        }
        InputType::Zip => {
            EnableWindow(ui.h_btn_browse, if ui.is_processing { 0 } else { 1 });
            set_cue(
                ui.h_edit_path,
                "点击右侧“浏览”或直接拖放本地 .zip 压缩包到此窗口...",
            );
            set_text(ui.h_btn_pack, "开始打包 (Pack)");
        }
        InputType::Restore => {
            EnableWindow(ui.h_btn_browse, if ui.is_processing { 0 } else { 1 });
            set_cue(
                ui.h_edit_path,
                "选择或拖放本工具生成的代码说明文件 (.md / .xml / .txt) 还原为 ZIP...",
            );
            set_text(ui.h_btn_pack, "还原为 ZIP");
        }
    }
}

unsafe fn set_processing_state(ui: &mut UiState, processing: bool) {
    ui.is_processing = processing;
    let en = if processing { 0 } else { 1 };
    EnableWindow(ui.h_radio_url, en);
    EnableWindow(ui.h_radio_dir, en);
    EnableWindow(ui.h_radio_zip, en);
    EnableWindow(ui.h_radio_restore, en);
    EnableWindow(ui.h_edit_path, en);
    EnableWindow(ui.h_chk_log, en);
    EnableWindow(ui.h_btn_pack, en);
    EnableWindow(ui.h_btn_reset, en);
    refresh_input_ui(ui);
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "codebase".to_string()
    } else {
        trimmed.to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_tokens(chars: usize) -> String {
    let tokens = (chars + 3) / 4;
    if tokens >= 1000 {
        format!("{:.1}K", tokens as f64 / 1000.0)
    } else {
        format!("{tokens}")
    }
}

fn looks_like_git_url(s: &str) -> bool {
    let lower = s.trim().to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("git@")
        || lower.starts_with("ssh://")
}

/// 解析 Git URL，兼容 GitHub/GitLab 网页分支链接，如：
/// https://github.com/user/repo/tree/dev 或 https://gitlab.com/user/repo/-/tree/dev
fn parse_git_url(raw: &str) -> (String, Option<String>) {
    let trimmed = raw.trim().trim_end_matches('/');

    if let Some(idx) = trimmed.find("/-/tree/") {
        let base = &trimmed[..idx];
        let rest = &trimmed[idx + "/-/tree/".len()..];
        let branch = rest.split('/').next().unwrap_or("").trim();
        if !branch.is_empty() {
            return (base.to_string(), Some(branch.to_string()));
        }
    }

    if let Some(idx) = trimmed.find("/tree/") {
        let base = &trimmed[..idx];
        let rest = &trimmed[idx + "/tree/".len()..];
        let branch = rest.split('/').next().unwrap_or("").trim();
        if !branch.is_empty() && base.matches('/').count() >= 4 {
            return (base.to_string(), Some(branch.to_string()));
        }
    }

    (trimmed.to_string(), None)
}

fn build_default_filename(input_type: InputType, input: &str, format: OutputFormat) -> String {
    let raw_name = match input_type {
        InputType::Url => {
            let (base_url, _) = parse_git_url(input);
            let last = base_url.trim_end_matches('/').rsplit('/').next().unwrap_or("codebase");
            last.strip_suffix(".git").unwrap_or(last).to_string()
        }
        InputType::Folder => Path::new(input)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "codebase".to_string()),
        InputType::Zip | InputType::Restore => Path::new(input)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "codebase".to_string()),
    };

    let base = sanitize_filename(&raw_name);
    if input_type == InputType::Restore {
        return format!("{base}_restored.zip");
    }

    let date = current_date_string();
    let ext = match format {
        OutputFormat::Xml => "xml",
        OutputFormat::Markdown => "md",
        OutputFormat::Plain => "txt",
    };

    format!("{base}_{date}.{ext}")
}

fn validate_before_save(input_type: InputType, input: &str) -> Result<(), String> {
    match input_type {
        InputType::Folder => {
            let p = Path::new(input);
            if !p.exists() || !p.is_dir() {
                return Err("指定的本地文件夹不存在，请重新确认路径。".to_string());
            }
        }
        InputType::Zip => {
            let p = Path::new(input);
            if !p.exists() || !p.is_file() {
                return Err("指定的 ZIP 文件不存在。".to_string());
            }
            if !input.to_ascii_lowercase().ends_with(".zip") {
                return Err("请选择有效的 .zip 压缩包文件。".to_string());
            }
        }
        InputType::Restore => {
            let p = Path::new(input);
            if !p.exists() || !p.is_file() {
                return Err("指定的代码说明文件不存在。".to_string());
            }
            let meta = fs::metadata(p).map_err(|e| format!("无法读取文件信息: {e}"))?;
            if meta.len() == 0 {
                return Err("输入的代码说明文件为空。".to_string());
            }
        }
        InputType::Url => {
            if !looks_like_git_url(input) {
                return Err(
                    "Git 链接格式无效，请以 https://、http://、git@ 或 ssh:// 开头。".to_string(),
                );
            }
            let git_check = Command::new("git")
                .arg("--version")
                .creation_flags(CREATE_NO_WINDOW)
                .output();
            match git_check {
                Ok(out) if out.status.success() => {}
                _ => {
                    return Err(
                        "未检测到可用的 Git 命令，请先在本机安装 Git 并添加到环境变量。".to_string(),
                    );
                }
            }
        }
    }
    Ok(())
}

struct ScanStats {
    candidates: Vec<(String, PathBuf)>,
    skipped_ignored_ext: usize,
    skipped_large: usize,
}

fn collect_files(root: &Path, dir: &Path, stats: &mut ScanStats) -> io::Result<()> {
    const IGNORED_DIRS: &[&str] = &[
        ".git",
        "node_modules",
        "build",
        "dist",
        "target",
        ".dart_tool",
        "__pycache__",
        ".venv",
        "venv",
        ".idea",
        ".vscode",
        ".vs",
        ".next",
        ".nuxt",
        "coverage",
        "out",
        ".gradle",
    ];
    const IGNORED_EXTS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "tiff", "svg", "zip", "tar", "gz",
        "7z", "rar", "xz", "bz2", "exe", "dll", "so", "dylib", "pdb", "lib", "a", "o", "obj",
        "class", "jar", "war", "pyc", "pyo", "wasm", "apk", "aab", "ipa", "pdf", "doc", "docx",
        "xls", "xlsx", "ppt", "pptx", "woff", "woff2", "ttf", "otf", "eot", "mp3", "mp4",
        "wav", "flac", "avi", "mov", "mkv", "sqlite", "db", "bin", "dat",
    ];
    const IGNORED_FILES: &[&str] = &[".ds_store", "thumbs.db", "desktop.ini"];

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if file_type.is_dir() {
            if IGNORED_DIRS.iter().any(|&d| name.eq_ignore_ascii_case(d)) {
                continue;
            }
            collect_files(root, &path, stats)?;
        } else if file_type.is_file() {
            let name_lower = name.to_ascii_lowercase();
            if IGNORED_FILES.contains(&name_lower.as_str()) {
                continue;
            }

            let ext_lower = path
                .extension()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if IGNORED_EXTS.contains(&ext_lower.as_str()) {
                stats.skipped_ignored_ext += 1;
                continue;
            }

            if let Ok(meta) = entry.metadata() {
                if meta.len() > MAX_SINGLE_FILE_BYTES {
                    stats.skipped_large += 1;
                    continue;
                }
            }

            if let Ok(rel) = path.strip_prefix(root) {
                let norm = rel.to_string_lossy().replace('\\', "/");
                stats.candidates.push((norm, path));
            }
        }
    }
    Ok(())
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    sample.contains(&0)
}

fn choose_fence(content: &str) -> String {
    let mut max_run = 0usize;
    let mut cur = 0usize;
    for b in content.bytes() {
        if b == b'`' {
            cur += 1;
            if cur > max_run {
                max_run = cur;
            }
        } else {
            cur = 0;
        }
    }
    "`".repeat((max_run + 1).max(3))
}

fn escape_xml_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn write_xml_cdata<W: Write>(w: &mut W, content: &str) -> io::Result<()> {
    w.write_all(b"<![CDATA[\n")?;
    let mut rest = content;
    while let Some(pos) = rest.find("]]>") {
        w.write_all(rest[..pos].as_bytes())?;
        w.write_all(b"]]]]><![CDATA[>")?;
        rest = &rest[pos + 3..];
    }
    w.write_all(rest.as_bytes())?;
    if !content.ends_with('\n') {
        w.write_all(b"\n")?;
    }
    w.write_all(b"]]>")
}

#[derive(Default)]
struct TreeNode {
    children: BTreeMap<String, TreeNode>,
    is_file: bool,
}

impl TreeNode {
    fn insert(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let mut curr = self;
        for (idx, part) in parts.iter().enumerate() {
            curr = curr.children.entry((*part).to_string()).or_default();
            if idx + 1 == parts.len() {
                curr.is_file = true;
            }
        }
    }

    fn write_tree<W: Write>(&self, w: &mut W, prefix: &str) -> io::Result<()> {
        let count = self.children.len();
        for (idx, (name, child)) in self.children.iter().enumerate() {
            let is_last = idx + 1 == count;
            let branch = if is_last { "└── " } else { "├── " };
            if child.children.is_empty() && child.is_file {
                writeln!(w, "{prefix}{branch}{name}")?;
            } else {
                writeln!(w, "{prefix}{branch}{name}/")?;
                let next_prefix = if is_last {
                    format!("{prefix}    ")
                } else {
                    format!("{prefix}│   ")
                };
                child.write_tree(w, &next_prefix)?;
            }
        }
        Ok(())
    }
}

fn extract_zip(zip_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| format!("无法打开 ZIP 文件: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(BufReader::new(file)).map_err(|e| format!("解析 ZIP 压缩包失败: {e}"))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取 ZIP 第 {i} 项失败（可能压缩方式受限或编码异常）: {e}"))?;
        let Some(rel_path) = entry.enclosed_name() else {
            write_log_line(
                "WARN",
                &format!(
                    "跳过非安全或非 UTF-8 路径的 ZIP 条目: {:?}",
                    String::from_utf8_lossy(entry.name_raw())
                ),
            );
            continue;
        };
        let out_path = dest_dir.join(rel_path);

        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| format!("创建目录失败: {e}"))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("创建父目录失败: {e}"))?;
            }
            let mut out_file =
                fs::File::create(&out_path).map_err(|e| format!("创建解压文件失败: {e}"))?;
            io::copy(&mut entry, &mut out_file).map_err(|e| format!("解压写入失败: {e}"))?;
        }
    }
    Ok(())
}

struct PackOutcome {
    packed_files: usize,
    skipped_total: usize,
    total_chars: usize,
    output_bytes: u64,
}

fn execute_pack(
    input_type: InputType,
    input: &str,
    output_path: &Path,
    format: OutputFormat,
    include_summary: bool,
    include_structure: bool,
) -> Result<PackOutcome, String> {
    let start_at = Instant::now();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    write_log_line(
        "INFO",
        &format!("开始正向打包: input={input}, output={}", output_path.display()),
    );

    let mut temp_dirs: Vec<PathBuf> = Vec::new();
    let body_temp_path = std::env::temp_dir().join(format!("repomix_body_{stamp}.tmp"));

    let result = (|| -> Result<PackOutcome, String> {
        let target_dir = match input_type {
            InputType::Folder => {
                let p = PathBuf::from(input);
                if !p.is_dir() {
                    return Err("指定的文件夹不存在".to_string());
                }
                p
            }
            InputType::Zip => {
                let dir = std::env::temp_dir().join(format!("repomix_zip_{stamp}"));
                fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败: {e}"))?;
                temp_dirs.push(dir.clone());
                extract_zip(Path::new(input), &dir)?;
                dir
            }
            InputType::Url => {
                let dir = std::env::temp_dir().join(format!("repomix_git_{stamp}"));
                temp_dirs.push(dir.clone());
                let (repo_url, branch_opt) = parse_git_url(input);
                let mut cmd = Command::new("git");
                cmd.args(["clone", "--depth", "1", "--single-branch"]);
                if let Some(ref b) = branch_opt {
                    cmd.args(["--branch", b]);
                }
                cmd.arg(&repo_url).arg(&dir).creation_flags(CREATE_NO_WINDOW);

                write_log_line(
                    "INFO",
                    &format!("执行 Git Clone: url={repo_url}, branch={branch_opt:?}"),
                );
                let output = cmd
                    .output()
                    .map_err(|e| format!("无法运行 git，请确认本机已安装 Git: {e}"))?;

                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    write_log_line("ERROR", &format!("Git Clone 失败: {}", err.trim()));
                    return Err(format!("Git Clone 失败: {}", err.trim()));
                }
                dir
            }
            InputType::Restore => return Err("内部模式冲突".to_string()),
        };

        let mut scan = ScanStats {
            candidates: Vec::new(),
            skipped_ignored_ext: 0,
            skipped_large: 0,
        };
        collect_files(&target_dir, &target_dir, &mut scan)
            .map_err(|e| format!("遍历目录失败: {e}"))?;
        scan.candidates.sort_by(|a, b| a.0.cmp(&b.0));

        // 流式写入临时正文文件，统计真实打包信息
        let body_file =
            File::create(&body_temp_path).map_err(|e| format!("创建临时缓存失败: {e}"))?;
        let mut body_writer = BufWriter::with_capacity(64 * 1024, body_file);

        let mut packed_paths: Vec<String> = Vec::with_capacity(scan.candidates.len());
        let mut skipped_binary_or_non_utf8 = 0usize;
        let mut total_lines = 0usize;
        let mut total_chars = 0usize;

        for (rel, full_path) in &scan.candidates {
            let Ok(bytes) = fs::read(full_path) else {
                skipped_binary_or_non_utf8 += 1;
                continue;
            };
            if is_probably_binary(&bytes) {
                skipped_binary_or_non_utf8 += 1;
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                skipped_binary_or_non_utf8 += 1;
                continue;
            };

            total_lines += text.lines().count();
            total_chars += text.chars().count();
            packed_paths.push(rel.clone());

            match format {
                OutputFormat::Xml => {
                    writeln!(body_writer, "<file path=\"{}\">", escape_xml_attr(rel))
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                    write_xml_cdata(&mut body_writer, &text)
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                    writeln!(body_writer, "\n</file>\n")
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                }
                OutputFormat::Markdown => {
                    let ext = full_path
                        .extension()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let fence = choose_fence(&text);
                    writeln!(body_writer, "### File: {rel}")
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                    writeln!(body_writer, "{fence}{ext}")
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                    body_writer
                        .write_all(text.as_bytes())
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                    if !text.ends_with('\n') {
                        body_writer
                            .write_all(b"\n")
                            .map_err(|e| format!("写入缓存失败: {e}"))?;
                    }
                    writeln!(body_writer, "{fence}\n")
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                }
                OutputFormat::Plain => {
                    writeln!(body_writer, "--- START: {rel} ---")
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                    body_writer
                        .write_all(text.as_bytes())
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                    if !text.ends_with('\n') {
                        body_writer
                            .write_all(b"\n")
                            .map_err(|e| format!("写入缓存失败: {e}"))?;
                    }
                    writeln!(body_writer, "--- END ---\n")
                        .map_err(|e| format!("写入缓存失败: {e}"))?;
                }
            }
        }
        body_writer
            .flush()
            .map_err(|e| format!("刷新缓存失败: {e}"))?;
        drop(body_writer);

        if packed_paths.is_empty() {
            return Err("未找到任何可打包的文本/源码文件。".to_string());
        }

        let skipped_total =
            scan.skipped_ignored_ext + scan.skipped_large + skipped_binary_or_non_utf8;

        // 写入最终输出文件（头部树形结构 + 汇总统计 + 流式拷贝正文）
        let out_file =
            File::create(output_path).map_err(|e| format!("创建输出文件失败: {e}"))?;
        let mut out_writer = BufWriter::with_capacity(64 * 1024, out_file);

        if include_structure {
            writeln!(out_writer, "=== DIRECTORY STRUCTURE ===")
                .map_err(|e| format!("写入输出失败: {e}"))?;
            let mut tree = TreeNode::default();
            for p in &packed_paths {
                tree.insert(p);
            }
            tree.write_tree(&mut out_writer, "")
                .map_err(|e| format!("写入输出失败: {e}"))?;
            writeln!(out_writer, "===========================\n")
                .map_err(|e| format!("写入输出失败: {e}"))?;
        }

        if include_summary {
            let fmt_name = match format {
                OutputFormat::Xml => "XML (CDATA)",
                OutputFormat::Markdown => "Markdown",
                OutputFormat::Plain => "Plain Text",
            };
            writeln!(out_writer, "=== FILE SUMMARY ===")
                .map_err(|e| format!("写入输出失败: {e}"))?;
            writeln!(out_writer, "Output Format   : {fmt_name}")
                .map_err(|e| format!("写入输出失败: {e}"))?;
            writeln!(out_writer, "Packed Files    : {}", packed_paths.len())
                .map_err(|e| format!("写入输出失败: {e}"))?;
            writeln!(
                out_writer,
                "Skipped Files   : {} (Ignored ext: {}, >2MB: {}, Binary/Non-UTF8: {})",
                skipped_total, scan.skipped_ignored_ext, scan.skipped_large, skipped_binary_or_non_utf8
            )
            .map_err(|e| format!("写入输出失败: {e}"))?;
            writeln!(out_writer, "Total Lines     : {total_lines}")
                .map_err(|e| format!("写入输出失败: {e}"))?;
            writeln!(out_writer, "Total Chars     : {total_chars}")
                .map_err(|e| format!("写入输出失败: {e}"))?;
            writeln!(out_writer, "Estimated Tokens: ~{}", format_tokens(total_chars))
                .map_err(|e| format!("写入输出失败: {e}"))?;
            writeln!(out_writer, "====================\n")
                .map_err(|e| format!("写入输出失败: {e}"))?;
        }

        writeln!(out_writer, "=== CODEBASE CONTENT ===\n")
            .map_err(|e| format!("写入输出失败: {e}"))?;

        let mut body_reader =
            File::open(&body_temp_path).map_err(|e| format!("读取临时缓存失败: {e}"))?;
        io::copy(&mut body_reader, &mut out_writer)
            .map_err(|e| format!("合并输出文件内容失败: {e}"))?;
        out_writer
            .flush()
            .map_err(|e| format!("保存输出文件失败: {e}"))?;

        let output_bytes = fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
        write_log_line(
            "INFO",
            &format!(
                "打包完成: files={}, skipped={}, chars={}, size={}, 耗时={}ms",
                packed_paths.len(),
                skipped_total,
                total_chars,
                format_bytes(output_bytes),
                start_at.elapsed().as_millis()
            ),
        );

        Ok(PackOutcome {
            packed_files: packed_paths.len(),
            skipped_total,
            total_chars,
            output_bytes,
        })
    })();

    let _ = fs::remove_file(&body_temp_path);
    for dir in temp_dirs {
        let _ = fs::remove_dir_all(dir);
    }

    result
}

// =========================================================================
// 反向功能：代码说明文档 (.md / .xml / .txt) 还原为标准 ZIP 压缩包
// =========================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum DocFormat {
    Markdown,
    Xml,
    Plain,
}

fn sanitize_archive_rel_path(raw: &str) -> Option<String> {
    let mut s = raw.trim().replace('\\', "/");
    // 剥离 Windows 盘符前缀 (如 C:/)
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        s = s[2..].to_string();
    }
    let parts: Vec<&str> = Path::new(&s)
        .components()
        .filter_map(|c| match c {
            Component::Normal(os) => {
                let part = os.to_string_lossy();
                if part.is_empty() || part == "." || part == ".." {
                    None
                } else {
                    Some(part.into_owned())
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .leak_free_skip();
    let _ = parts;
    None
}

fn clean_rel_path(raw: &str) -> Option<String> {
    let mut s = raw.trim().replace('\\', "/");
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        s = s[2..].to_string();
    }
    let mut clean_parts: Vec<String> = Vec::new();
    for seg in s.split('/') {
        let seg = seg.trim();
        if seg.is_empty() || seg == "." || seg == ".." {
            continue;
        }
        let safe_seg: String = seg
            .chars()
            .map(|c| match c {
                '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
                _ => c,
            })
            .collect();
        if !safe_seg.is_empty() {
            clean_parts.push(safe_seg);
        }
    }
    if clean_parts.is_empty() {
        None
    } else {
        Some(clean_parts.join("/"))
    }
}

trait UnusedHelper {
    fn leak_free_skip(self) -> Self;
}
impl UnusedHelper for Vec<String> {
    fn leak_free_skip(self) -> Self {
        self
    }
}

fn unescape_xml_entities(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn detect_doc_format(doc_path: &Path) -> Result<DocFormat, String> {
    let file = File::open(doc_path).map_err(|e| format!("无法打开文档文件: {e}"))?;
    let reader = BufReader::new(file);
    for line in reader.lines().take(400) {
        let Ok(l) = line else {
            continue;
        };
        let t = l.trim();
        if t.starts_with("### File:") || t.starts_with("## File:") {
            return Ok(DocFormat::Markdown);
        }
        if t.starts_with("<file path=\"") {
            return Ok(DocFormat::Xml);
        }
        if t.starts_with("--- START:") && t.ends_with("---") {
            return Ok(DocFormat::Plain);
        }
    }

    match doc_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("xml") => Ok(DocFormat::Xml),
        Some("txt") => Ok(DocFormat::Plain),
        _ => Ok(DocFormat::Markdown),
    }
}

fn write_restored_entry(
    zip: &mut ZipWriter<BufWriter<File>>,
    seen_paths: &mut HashSet<String>,
    raw_path: &str,
    content: &str,
) -> Result<bool, String> {
    let Some(rel_path) = clean_rel_path(raw_path) else {
        write_log_line("WARN", &format!("跳过无效还原路径: {raw_path}"));
        return Ok(false);
    };
    if !seen_paths.insert(rel_path.clone()) {
        write_log_line("WARN", &format!("跳过重复文件路径: {rel_path}"));
        return Ok(false);
    }

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(&rel_path, options)
        .map_err(|e| format!("写入 ZIP 条目 {rel_path} 失败: {e}"))?;
    zip.write_all(content.as_bytes())
        .map_err(|e| format!("写入 ZIP 内容 {rel_path} 失败: {e}"))?;
    Ok(true)
}

fn decode_xml_body(raw_body: &str) -> String {
    let trimmed = raw_body.trim_matches(|c| c == '\r' || c == '\n');
    if let Some(after_start) = trimmed.strip_prefix("<![CDATA[") {
        if let Some(inner) = after_start.strip_suffix("]]>") {
            let inner = inner.strip_prefix('\n').unwrap_or(inner);
            let inner = inner.strip_suffix('\n').unwrap_or(inner);
            return inner.replace("]]]]><![CDATA[>", "]]>");
        }
    }
    raw_body.to_string()
}

fn execute_restore_to_zip(doc_path: &Path, zip_out_path: &Path) -> Result<(usize, u64), String> {
    let start_at = Instant::now();
    let fmt = detect_doc_format(doc_path)?;
    write_log_line(
        "INFO",
        &format!(
            "开始反向还原 ZIP: doc={}, out={}, detected_fmt={}",
            doc_path.display(),
            zip_out_path.display(),
            match fmt {
                DocFormat::Markdown => "Markdown",
                DocFormat::Xml => "XML",
                DocFormat::Plain => "Plain",
            }
        ),
    );

    let in_file = File::open(doc_path).map_err(|e| format!("无法打开输入文档: {e}"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, in_file);

    let out_file = File::create(zip_out_path).map_err(|e| format!("无法创建 ZIP 文件: {e}"))?;
    let mut zip = ZipWriter::new(BufWriter::with_capacity(64 * 1024, out_file));
    let mut seen_paths = HashSet::new();
    let mut restored_count = 0usize;

    let mut line_buf = String::new();

    match fmt {
        DocFormat::Markdown => {
            enum MdState {
                SeekingHeader,
                ExpectFence(String),
                InBody(String, usize, String),
            }
            let mut state = MdState::SeekingHeader;

            loop {
                line_buf.clear();
                let n = reader
                    .read_line(&mut line_buf)
                    .map_err(|e| format!("读取文档失败: {e}"))?;
                if n == 0 {
                    break;
                }
                let line_no_cr = line_buf.trim_end_matches(&['\r', '\n'][..]);

                match &mut state {
                    MdState::SeekingHeader => {
                        let trimmed = line_no_cr.trim();
                        let path_opt = trimmed
                            .strip_prefix("### File:")
                            .or_else(|| trimmed.strip_prefix("## File:"));
                        if let Some(p) = path_opt {
                            let path_str = p.trim().to_string();
                            if !path_str.is_empty() {
                                state = MdState::ExpectFence(path_str);
                            }
                        }
                    }
                    MdState::ExpectFence(path_str) => {
                        let trimmed = line_no_cr.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let tick_count = trimmed.bytes().take_while(|&b| b == b'`').count();
                        if tick_count >= 3 {
                            let target_path = std::mem::take(path_str);
                            state = MdState::InBody(
                                target_path,
                                tick_count,
                                String::with_capacity(4096),
                            );
                        } else {
                            state = MdState::SeekingHeader;
                        }
                    }
                    MdState::InBody(target_path, fence_len, content) => {
                        let trimmed = line_no_cr.trim();
                        if trimmed.len() == *fence_len && trimmed.bytes().all(|b| b == b'`') {
                            let final_path = std::mem::take(target_path);
                            let final_body = std::mem::take(content);
                            if write_restored_entry(
                                &mut zip,
                                &mut seen_paths,
                                &final_path,
                                &final_body,
                            )? {
                                restored_count += 1;
                            }
                            state = MdState::SeekingHeader;
                        } else {
                            content.push_str(line_no_cr);
                            content.push('\n');
                        }
                    }
                }
            }
        }
        DocFormat::Xml => {
            let mut cur_path: Option<String> = None;
            let mut cur_body = String::new();

            loop {
                line_buf.clear();
                let n = reader
                    .read_line(&mut line_buf)
                    .map_err(|e| format!("读取文档失败: {e}"))?;
                if n == 0 {
                    break;
                }
                let line_no_cr = line_buf.trim_end_matches(&['\r', '\n'][..]);

                if cur_path.is_none() {
                    let trimmed = line_no_cr.trim();
                    if let Some(rest) = trimmed.strip_prefix("<file path=\"") {
                        if let Some(end_quote) = rest.find("\">") {
                            let raw_path = unescape_xml_entities(&rest[..end_quote]);
                            cur_path = Some(raw_path);
                            cur_body.clear();
                            let trailing = &rest[end_quote + 2..];
                            if !trailing.is_empty() {
                                if let Some(close_idx) = trailing.find("</file>") {
                                    let decoded = decode_xml_body(&trailing[..close_idx]);
                                    if let Some(p) = cur_path.take() {
                                        if write_restored_entry(
                                            &mut zip,
                                            &mut seen_paths,
                                            &p,
                                            &decoded,
                                        )? {
                                            restored_count += 1;
                                        }
                                    }
                                } else {
                                    cur_body.push_str(trailing);
                                    cur_body.push('\n');
                                }
                            }
                        }
                    }
                } else if line_no_cr.trim() == "</file>" {
                    if let Some(p) = cur_path.take() {
                        let decoded = decode_xml_body(&cur_body);
                        if write_restored_entry(&mut zip, &mut seen_paths, &p, &decoded)? {
                            restored_count += 1;
                        }
                        cur_body.clear();
                    }
                } else {
                    cur_body.push_str(line_no_cr);
                    cur_body.push('\n');
                }
            }
        }
        DocFormat::Plain => {
            let mut cur_path: Option<String> = None;
            let mut cur_body = String::new();

            loop {
                line_buf.clear();
                let n = reader
                    .read_line(&mut line_buf)
                    .map_err(|e| format!("读取文档失败: {e}"))?;
                if n == 0 {
                    break;
                }
                let line_no_cr = line_buf.trim_end_matches(&['\r', '\n'][..]);

                if cur_path.is_none() {
                    let trimmed = line_no_cr.trim();
                    if let Some(rest) = trimmed.strip_prefix("--- START:") {
                        if let Some(path_part) = rest.strip_suffix("---") {
                            let p = path_part.trim();
                            if !p.is_empty() {
                                cur_path = Some(p.to_string());
                                cur_body.clear();
                            }
                        }
                    }
                } else if line_no_cr.trim() == "--- END ---" {
                    if let Some(p) = cur_path.take() {
                        if write_restored_entry(&mut zip, &mut seen_paths, &p, &cur_body)? {
                            restored_count += 1;
                        }
                        cur_body.clear();
                    }
                } else {
                    cur_body.push_str(line_no_cr);
                    cur_body.push('\n');
                }
            }
        }
    }

    if restored_count == 0 {
        drop(zip);
        let _ = fs::remove_file(zip_out_path);
        return Err("未能从该文档中解析出任何有效代码文件，请确认这是本工具生成的说明文件。".to_string());
    }

    zip.finish().map_err(|e| format!("完成 ZIP 归档失败: {e}"))?;
    let zip_bytes = fs::metadata(zip_out_path).map(|m| m.len()).unwrap_or(0);

    write_log_line(
        "INFO",
        &format!(
            "反向还原完成: files={}, zip_size={}, 耗时={}ms",
            restored_count,
            format_bytes(zip_bytes),
            start_at.elapsed().as_millis()
        ),
    );
    Ok((restored_count, zip_bytes))
}

unsafe fn create_ctrl(
    class: &str,
    text: &str,
    style: u32,
    ex_style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: HWND,
    id: i32,
    h_inst: *mut c_void,
    font: HFONT,
) -> HWND {
    let cls_w = to_wide(class);
    let txt_w = to_wide(text);
    let hwnd = CreateWindowExW(
        ex_style,
        cls_w.as_ptr(),
        txt_w.as_ptr(),
        style,
        x,
        y,
        w,
        h,
        parent,
        id as isize as HMENU,
        h_inst,
        null_mut(),
    );
    if !hwnd.is_null() {
        SendMessageW(hwnd, WM_SETFONT, font as usize, 1);
    }
    hwnd
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            DragAcceptFiles(hwnd, 1);

            let h_inst = GetModuleHandleW(null_mut());
            let icon = LoadIconW(h_inst, 1usize as *const u16);
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as LPARAM);
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon as LPARAM);
            }

            let hdc = GetDC(null_mut());
            let dpi = if !hdc.is_null() {
                let d = GetDeviceCaps(hdc, LOGPIXELSX);
                ReleaseDC(null_mut(), hdc);
                if d > 0 { d } else { 96 }
            } else {
                96
            };

            let font_name = to_wide("Microsoft YaHei UI");
            let font_normal = CreateFontW(
                -scale(14, dpi),
                0,
                0,
                0,
                FW_NORMAL as _,
                0,
                0,
                0,
                DEFAULT_CHARSET as _,
                OUT_DEFAULT_PRECIS as _,
                CLIP_DEFAULT_PRECIS as _,
                CLEARTYPE_QUALITY as _,
                (DEFAULT_PITCH | FF_DONTCARE) as _,
                font_name.as_ptr(),
            );
            let font_title = CreateFontW(
                -scale(20, dpi),
                0,
                0,
                0,
                FW_BOLD as _,
                0,
                0,
                0,
                DEFAULT_CHARSET as _,
                OUT_DEFAULT_PRECIS as _,
                CLIP_DEFAULT_PRECIS as _,
                CLEARTYPE_QUALITY as _,
                (DEFAULT_PITCH | FF_DONTCARE) as _,
                font_name.as_ptr(),
            );
            let bg_brush = CreateSolidBrush(0x00F5F5F5);

            let s = |v: i32| scale(v, dpi);

            create_ctrl(
                "STATIC",
                "Repomix Portable",
                WS_CHILD | WS_VISIBLE,
                0,
                s(18),
                s(12),
                s(380),
                s(26),
                hwnd,
                0,
                h_inst,
                font_title,
            );
            create_ctrl(
                "STATIC",
                "支持仓库打包为单文档，或将说明文档反向还原为 ZIP（纯绿色免安装）",
                WS_CHILD | WS_VISIBLE,
                0,
                s(18),
                s(40),
                s(540),
                s(18),
                hwnd,
                0,
                h_inst,
                font_normal,
            );

            create_ctrl(
                "BUTTON",
                "模式与输入来源",
                WS_CHILD | WS_VISIBLE | BS_GROUPBOX,
                0,
                s(18),
                s(64),
                s(552),
                s(90),
                hwnd,
                0,
                h_inst,
                font_normal,
            );

            let h_radio_url = create_ctrl(
                "BUTTON",
                "Git 链接",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_GROUP | BS_AUTORADIOBUTTON,
                0,
                s(32),
                s(85),
                s(78),
                s(22),
                hwnd,
                ID_RADIO_URL,
                h_inst,
                font_normal,
            );
            let h_radio_dir = create_ctrl(
                "BUTTON",
                "本地文件夹",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTORADIOBUTTON,
                0,
                s(118),
                s(85),
                s(96),
                s(22),
                hwnd,
                ID_RADIO_DIR,
                h_inst,
                font_normal,
            );
            let h_radio_zip = create_ctrl(
                "BUTTON",
                "ZIP 压缩包",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTORADIOBUTTON,
                0,
                s(222),
                s(85),
                s(98),
                s(22),
                hwnd,
                ID_RADIO_ZIP,
                h_inst,
                font_normal,
            );
            let h_radio_restore = create_ctrl(
                "BUTTON",
                "文档还原 (转ZIP)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTORADIOBUTTON,
                0,
                s(328),
                s(85),
                s(140),
                s(22),
                hwnd,
                ID_RADIO_RESTORE,
                h_inst,
                font_normal,
            );

            let h_edit_path = create_ctrl(
                "EDIT",
                "",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL,
                WS_EX_CLIENTEDGE,
                s(32),
                s(114),
                s(440),
                s(24),
                hwnd,
                ID_EDIT_PATH,
                h_inst,
                font_normal,
            );
            let h_btn_browse = create_ctrl(
                "BUTTON",
                "浏览...",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                0,
                s(480),
                s(113),
                s(76),
                s(26),
                hwnd,
                ID_BTN_BROWSE,
                h_inst,
                font_normal,
            );

            create_ctrl(
                "BUTTON",
                "输出格式",
                WS_CHILD | WS_VISIBLE | WS_GROUP | BS_GROUPBOX,
                0,
                s(18),
                s(164),
                s(258),
                s(68),
                hwnd,
                0,
                h_inst,
                font_normal,
            );

            let h_radio_xml = create_ctrl(
                "BUTTON",
                "XML",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_GROUP | BS_AUTORADIOBUTTON,
                0,
                s(32),
                s(192),
                s(58),
                s(22),
                hwnd,
                ID_RADIO_XML,
                h_inst,
                font_normal,
            );
            let h_radio_md = create_ctrl(
                "BUTTON",
                "Markdown",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTORADIOBUTTON,
                0,
                s(100),
                s(192),
                s(90),
                s(22),
                hwnd,
                ID_RADIO_MD,
                h_inst,
                font_normal,
            );
            let h_radio_plain = create_ctrl(
                "BUTTON",
                "Plain",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTORADIOBUTTON,
                0,
                s(198),
                s(192),
                s(64),
                s(22),
                hwnd,
                ID_RADIO_PLAIN,
                h_inst,
                font_normal,
            );

            create_ctrl(
                "BUTTON",
                "内容选项",
                WS_CHILD | WS_VISIBLE | WS_GROUP | BS_GROUPBOX,
                0,
                s(286),
                s(164),
                s(284),
                s(68),
                hwnd,
                0,
                h_inst,
                font_normal,
            );

            let h_chk_summary = create_ctrl(
                "BUTTON",
                "包含统计摘要 (File Summary)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_GROUP | BS_AUTOCHECKBOX,
                0,
                s(300),
                s(185),
                s(252),
                s(20),
                hwnd,
                ID_CHK_SUMMARY,
                h_inst,
                font_normal,
            );
            let h_chk_structure = create_ctrl(
                "BUTTON",
                "包含目录树 (Directory Structure)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                0,
                s(300),
                s(207),
                s(258),
                s(20),
                hwnd,
                ID_CHK_STRUCTURE,
                h_inst,
                font_normal,
            );

            let h_chk_log = create_ctrl(
                "BUTTON",
                "启用运行日志 (默认关闭，勾选后写入同级目录)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                0,
                s(18),
                s(249),
                s(336),
                s(22),
                hwnd,
                ID_CHK_LOG,
                h_inst,
                font_normal,
            );

            let h_btn_pack = create_ctrl(
                "BUTTON",
                "开始打包 (Pack)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
                0,
                s(368),
                s(244),
                s(124),
                s(32),
                hwnd,
                ID_BTN_PACK,
                h_inst,
                font_normal,
            );
            let h_btn_reset = create_ctrl(
                "BUTTON",
                "重置",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                0,
                s(500),
                s(244),
                s(70),
                s(32),
                hwnd,
                ID_BTN_RESET,
                h_inst,
                font_normal,
            );

            let h_lbl_status = create_ctrl(
                "STATIC",
                "就绪。支持直接拖放文件夹、.zip 或 .md/.xml/.txt 文档。",
                WS_CHILD | WS_VISIBLE,
                0,
                s(18),
                s(286),
                s(552),
                s(36),
                hwnd,
                ID_LBL_STATUS,
                h_inst,
                font_normal,
            );

            set_checked(h_radio_dir, true);
            set_checked(h_radio_md, true);
            set_checked(h_chk_summary, true);
            set_checked(h_chk_structure, true);
            set_checked(h_chk_log, STARTUP_LOG_ARG.load(Ordering::Relaxed));

            let state = UiState {
                h_radio_url,
                h_radio_dir,
                h_radio_zip,
                h_radio_restore,
                h_edit_path,
                h_btn_browse,
                h_radio_xml,
                h_radio_md,
                h_radio_plain,
                h_chk_summary,
                h_chk_structure,
                h_chk_log,
                h_btn_pack,
                h_btn_reset,
                h_lbl_status,
                font_normal,
                font_title,
                bg_brush,
                is_processing: false,
                suppress_edit_change: false,
            };
            refresh_input_ui(&state);

            write_log_line("INFO", &format!("主窗口创建成功 (DPI={dpi})"));

            UI.with(|cell| {
                *cell.borrow_mut() = Some(state);
            });
            0
        }

        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let hdc = wparam as HDC;
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, 0x00222222);
            UI.with(|cell| {
                if let Some(ui) = cell.borrow().as_ref() {
                    ui.bg_brush as LRESULT
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            })
        }

        WM_DROPFILES => {
            let hdrop = wparam as HDROP;
            let len = DragQueryFileW(hdrop, 0, null_mut(), 0);
            if len > 0 {
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(hdrop, 0, buf.as_mut_ptr(), buf.len() as u32);
                let path = String::from_utf16_lossy(&buf[..len as usize]);

                UI.with(|cell| {
                    if let Some(ui) = cell.borrow_mut().as_mut() {
                        if ui.is_processing {
                            return;
                        }
                        let p = Path::new(&path);
                        let lower = path.to_ascii_lowercase();
                        if p.is_dir() {
                            set_input_type_radio(ui, InputType::Folder);
                            set_edit_text_silently(ui, &path);
                            refresh_input_ui(ui);
                            set_text(ui.h_lbl_status, "已加载拖放的文件夹。");
                            write_log_line("INFO", &format!("拖放文件夹: {path}"));
                        } else if lower.ends_with(".zip") {
                            set_input_type_radio(ui, InputType::Zip);
                            set_edit_text_silently(ui, &path);
                            refresh_input_ui(ui);
                            set_text(ui.h_lbl_status, "已加载拖放的 ZIP 压缩包。");
                            write_log_line("INFO", &format!("拖放 ZIP: {path}"));
                        } else if lower.ends_with(".md")
                            || lower.ends_with(".xml")
                            || lower.ends_with(".txt")
                        {
                            set_input_type_radio(ui, InputType::Restore);
                            set_edit_text_silently(ui, &path);
                            refresh_input_ui(ui);
                            set_text(ui.h_lbl_status, "已识别为代码说明文档，可点击“还原为 ZIP”。");
                            write_log_line("INFO", &format!("拖放还原文档: {path}"));
                        } else {
                            set_text(
                                ui.h_lbl_status,
                                "仅支持拖放文件夹、.zip 压缩包或 .md/.xml/.txt 说明文档。",
                            );
                        }
                    }
                });
            }
            DragFinish(hdrop);
            0
        }

        WM_COMMAND => {
            let ctrl_id = (wparam & 0xFFFF) as i32;
            let notify = ((wparam >> 16) & 0xFFFF) as u32;

            if ctrl_id == ID_EDIT_PATH && notify == EN_CHANGE {
                UI.with(|cell| {
                    if let Some(ui) = cell.borrow_mut().as_mut() {
                        if ui.suppress_edit_change || ui.is_processing {
                            return;
                        }
                        let text = get_text(ui.h_edit_path);
                        if looks_like_git_url(&text) && get_input_type(ui) != InputType::Url {
                            set_input_type_radio(ui, InputType::Url);
                            refresh_input_ui(ui);
                            set_text(ui.h_lbl_status, "检测到 Git 链接，已自动切换至 Git 模式。");
                        }
                    }
                });
                return 0;
            }

            if notify == BN_CLICKED {
                match ctrl_id {
                    ID_RADIO_URL | ID_RADIO_DIR | ID_RADIO_ZIP | ID_RADIO_RESTORE => {
                        UI.with(|cell| {
                            if let Some(ui) = cell.borrow().as_ref() {
                                refresh_input_ui(ui);
                            }
                        });
                    }
                    ID_CHK_LOG => {
                        UI.with(|cell| {
                            if let Some(ui) = cell.borrow().as_ref() {
                                let enabled = is_checked(ui.h_chk_log);
                                set_log_enabled(enabled);
                                if enabled {
                                    set_text(
                                        ui.h_lbl_status,
                                        "已开启运行日志：输出至程序同级目录 repomix-portable.log",
                                    );
                                } else {
                                    set_text(ui.h_lbl_status, "已关闭运行日志（不生成日志文件）。");
                                }
                            }
                        });
                    }
                    ID_BTN_BROWSE => {
                        let input_type =
                            UI.with(|cell| cell.borrow().as_ref().map(|ui| get_input_type(ui)));
                        if let Some(t) = input_type {
                            let picked = match t {
                                InputType::Folder => {
                                    FileDialog::new().set_title("选择项目文件夹").pick_folder()
                                }
                                InputType::Zip => FileDialog::new()
                                    .set_title("选择项目 ZIP 压缩包")
                                    .add_filter("ZIP 压缩包", &["zip"])
                                    .pick_file(),
                                InputType::Restore => FileDialog::new()
                                    .set_title("选择代码说明文件")
                                    .add_filter("说明文档", &["md", "xml", "txt"])
                                    .pick_file(),
                                InputType::Url => None,
                            };
                            if let Some(path) = picked {
                                UI.with(|cell| {
                                    if let Some(ui) = cell.borrow_mut().as_mut() {
                                        let p_str = path.to_string_lossy().into_owned();
                                        set_edit_text_silently(ui, &p_str);
                                        write_log_line("INFO", &format!("浏览选择输入: {p_str}"));
                                    }
                                });
                            }
                        }
                    }
                    ID_BTN_RESET => {
                        UI.with(|cell| {
                            if let Some(ui) = cell.borrow_mut().as_mut() {
                                if !ui.is_processing {
                                    set_edit_text_silently(ui, "");
                                    set_text(ui.h_lbl_status, "状态已重置。");
                                }
                            }
                        });
                    }
                    ID_BTN_PACK => {
                        let params = UI.with(|cell| {
                            let mut guard = cell.borrow_mut();
                            let ui = guard.as_mut()?;
                            if ui.is_processing {
                                return None;
                            }
                            let input = get_text(ui.h_edit_path);
                            if input.is_empty() {
                                set_text(
                                    ui.h_lbl_status,
                                    "请先选择文件夹、ZIP、说明文档或输入 Git 链接。",
                                );
                                return None;
                            }
                            let input_type = get_input_type(ui);
                            let format = get_output_format(ui);
                            let include_summary = is_checked(ui.h_chk_summary);
                            let include_structure = is_checked(ui.h_chk_structure);
                            Some((input_type, input, format, include_summary, include_structure))
                        });

                        if let Some((input_type, input, format, inc_sum, inc_str)) = params {
                            if let Err(err_msg) = validate_before_save(input_type, &input) {
                                write_log_line("WARN", &format!("前置校验未通过: {err_msg}"));
                                UI.with(|cell| {
                                    if let Some(ui) = cell.borrow().as_ref() {
                                        set_text(ui.h_lbl_status, &err_msg);
                                    }
                                });
                                show_error_box(hwnd, "输入校验失败", &err_msg);
                                return 0;
                            }

                            let default_name = build_default_filename(input_type, &input, format);
                            let (dialog_title, filter_label, ext) = if input_type == InputType::Restore {
                                ("保存还原后的 ZIP 压缩包", "ZIP 压缩包", "zip")
                            } else {
                                match format {
                                    OutputFormat::Xml => ("保存打包结果", "XML 文件", "xml"),
                                    OutputFormat::Markdown => ("保存打包结果", "Markdown 文件", "md"),
                                    OutputFormat::Plain => ("保存打包结果", "纯文本文件", "txt"),
                                }
                            };

                            if let Some(mut save_path) = FileDialog::new()
                                .set_title(dialog_title)
                                .set_file_name(&default_name)
                                .add_filter(filter_label, &[ext])
                                .save_file()
                            {
                                if save_path.extension().is_none() {
                                    save_path.set_extension(ext);
                                }

                                UI.with(|cell| {
                                    if let Some(ui) = cell.borrow_mut().as_mut() {
                                        set_processing_state(ui, true);
                                        let status = if input_type == InputType::Restore {
                                            "正在解析说明文档并生成 ZIP，请稍候..."
                                        } else {
                                            "正在打包中，请稍候..."
                                        };
                                        set_text(ui.h_lbl_status, status);
                                    }
                                });

                                let hwnd_val = hwnd as usize;
                                thread::spawn(move || {
                                    let msg_tuple = if input_type == InputType::Restore {
                                        match execute_restore_to_zip(Path::new(&input), &save_path) {
                                            Ok((count, zip_bytes)) => {
                                                let fname = save_path
                                                    .file_name()
                                                    .map(|s| s.to_string_lossy().into_owned())
                                                    .unwrap_or_default();
                                                (
                                                    true,
                                                    format!(
                                                        "还原成功（共 {count} 个文件，{}）：{fname}",
                                                        format_bytes(zip_bytes)
                                                    ),
                                                )
                                            }
                                            Err(e) => (false, e),
                                        }
                                    } else {
                                        match execute_pack(
                                            input_type,
                                            &input,
                                            &save_path,
                                            format,
                                            inc_sum,
                                            inc_str,
                                        ) {
                                            Ok(outcome) => {
                                                let fname = save_path
                                                    .file_name()
                                                    .map(|s| s.to_string_lossy().into_owned())
                                                    .unwrap_or_default();
                                                (
                                                    true,
                                                    format!(
                                                        "打包成功（{} 个文件，跳过 {}，约 {} tokens，{}）：{}",
                                                        outcome.packed_files,
                                                        outcome.skipped_total,
                                                        format_tokens(outcome.total_chars),
                                                        format_bytes(outcome.output_bytes),
                                                        fname
                                                    ),
                                                )
                                            }
                                            Err(e) => (false, e),
                                        }
                                    };

                                    let lock = WORK_RESULT.get_or_init(|| Mutex::new(None));
                                    if let Ok(mut slot) = lock.lock() {
                                        *slot = Some(msg_tuple);
                                    }
                                    PostMessageW(hwnd_val as HWND, WM_APP_DONE, 0, 0);
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            0
        }

        WM_APP_DONE => {
            let result = WORK_RESULT
                .get()
                .and_then(|m| m.lock().ok())
                .and_then(|mut s| s.take())
                .unwrap_or((true, "处理完成。".to_string()));

            UI.with(|cell| {
                if let Some(ui) = cell.borrow_mut().as_mut() {
                    set_processing_state(ui, false);
                    if result.0 {
                        set_text(ui.h_lbl_status, &result.1);
                    } else {
                        let err_display = format!("错误：{}", result.1);
                        set_text(ui.h_lbl_status, &err_display);
                    }
                }
            });

            if !result.0 {
                write_log_line("ERROR", &result.1);
                show_error_box(hwnd, "执行失败", &result.1);
            }
            0
        }

        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }

        WM_DESTROY => {
            write_log_line("INFO", "程序正常退出");
            UI.with(|cell| {
                if let Some(ui) = cell.borrow_mut().take() {
                    DeleteObject(ui.font_normal);
                    DeleteObject(ui.font_title);
                    DeleteObject(ui.bg_brush);
                }
            });
            PostQuitMessage(0);
            0
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    if std::env::args().any(|a| a == "--log") {
        STARTUP_LOG_ARG.store(true, Ordering::Relaxed);
        set_log_enabled(true);
    }

    std::panic::set_hook(Box::new(|info| {
        let msg = format!("发生未捕获异常: {info}");
        write_log_line("PANIC", &msg);
        unsafe {
            show_error_box(null_mut(), "程序异常", &msg);
        }
    }));

    unsafe {
        enable_modern_ui_and_dpi();

        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&icc);

        let h_inst = GetModuleHandleW(null_mut());
        let class_name = to_wide("RepomixPortableWin32");
        let bg = CreateSolidBrush(0x00F5F5F5);
        let h_icon: HICON = LoadIconW(h_inst, 1usize as *const u16);

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: h_inst,
            hIcon: h_icon,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: bg,
            lpszMenuName: null_mut(),
            lpszClassName: class_name.as_ptr(),
        };

        if RegisterClassW(&wc) == 0 {
            let err = io::Error::last_os_error();
            write_log_line("ERROR", &format!("RegisterClassW 失败: {err}"));
            DeleteObject(bg);
            return;
        }

        let hdc = GetDC(null_mut());
        let dpi = if !hdc.is_null() {
            let d = GetDeviceCaps(hdc, LOGPIXELSX);
            ReleaseDC(null_mut(), hdc);
            if d > 0 { d } else { 96 }
        } else {
            96
        };

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let ex_style = WS_EX_ACCEPTFILES;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: scale(588, dpi),
            bottom: scale(334, dpi),
        };
        AdjustWindowRectEx(&mut rect, style, 0, ex_style);

        let win_w = rect.right - rect.left;
        let win_h = rect.bottom - rect.top;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let pos_x = (screen_w - win_w) / 2;
        let pos_y = (screen_h - win_h) / 2;

        let title = to_wide("Repomix Portable");

        let hwnd = CreateWindowExW(
            ex_style,
            class_name.as_ptr(),
            title.as_ptr(),
            style,
            pos_x,
            pos_y,
            win_w,
            win_h,
            null_mut(),
            null_mut(),
            h_inst,
            null_mut(),
        );

        if hwnd.is_null() {
            let err = io::Error::last_os_error();
            write_log_line("ERROR", &format!("CreateWindowExW 失败: {err}"));
            DeleteObject(bg);
            return;
        }

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        DeleteObject(bg);
    }
}
