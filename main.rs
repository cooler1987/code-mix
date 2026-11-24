#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::ffi::c_void;
use std::fs;
use std::io::{self, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rfd::FileDialog;
use windows_sys::Win32::Foundation::{
    HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, SYSTEMTIME, WPARAM,
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
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnableWindow, GetMessageW,
    GetSystemMetrics, GetWindowTextLengthW, GetWindowTextW, LoadCursorW, PostMessageW,
    PostQuitMessage, RegisterClassW, SendMessageW, SetProcessDPIAware, SetWindowTextW,
    ShowWindow, TranslateMessage, UpdateWindow, BM_GETCHECK, BM_SETCHECK, BN_CLICKED,
    BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON,
    CS_HREDRAW, CS_VREDRAW, ES_AUTOHSCROLL, HMENU, IDC_ARROW, MSG, SM_CXSCREEN, SM_CYSCREEN,
    SW_SHOW, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLORSTATIC, WM_DESTROY,
    WM_DROPFILES, WM_SETFONT, WM_USER, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_EX_ACCEPTFILES, WS_EX_CLIENTEDGE, WS_GROUP, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
};

const ID_RADIO_URL: i32 = 101;
const ID_RADIO_DIR: i32 = 102;
const ID_RADIO_ZIP: i32 = 103;
const ID_EDIT_PATH: i32 = 104;
const ID_BTN_BROWSE: i32 = 105;
const ID_RADIO_XML: i32 = 106;
const ID_RADIO_MD: i32 = 107;
const ID_RADIO_PLAIN: i32 = 108;
const ID_CHK_SUMMARY: i32 = 109;
const ID_CHK_STRUCTURE: i32 = 110;
const ID_BTN_PACK: i32 = 111;
const ID_BTN_RESET: i32 = 112;
const ID_LBL_STATUS: i32 = 113;

const WM_APP_DONE: u32 = WM_USER + 1;
const EM_SETCUEBANNER: u32 = 0x1501;
const BST_UNCHECKED: usize = 0;
const BST_CHECKED: usize = 1;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputType {
    Url,
    Folder,
    Zip,
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
    h_edit_path: HWND,
    h_btn_browse: HWND,
    h_radio_xml: HWND,
    h_radio_md: HWND,
    h_radio_plain: HWND,
    h_chk_summary: HWND,
    h_chk_structure: HWND,
    h_btn_pack: HWND,
    h_btn_reset: HWND,
    h_lbl_status: HWND,
    font_normal: HFONT,
    font_title: HFONT,
    bg_brush: HBRUSH,
    is_processing: bool,
}

thread_local! {
    static UI: RefCell<Option<UiState>> = const { RefCell::new(None) };
}

static PACK_RESULT: OnceLock<Mutex<Option<(bool, String)>>> = OnceLock::new();

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn scale(val: i32, dpi: i32) -> i32 {
    (val * dpi + 48) / 96
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

unsafe fn get_text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..copied as usize]).trim().to_string()
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
    } else {
        InputType::Folder
    }
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
    match get_input_type(ui) {
        InputType::Url => {
            EnableWindow(ui.h_btn_browse, 0);
            set_cue(ui.h_edit_path, "请输入 GitHub 仓库克隆链接，如 https://github.com/user/repo");
        }
        InputType::Folder => {
            EnableWindow(ui.h_btn_browse, 1);
            set_cue(ui.h_edit_path, "点击右侧“浏览”或直接拖放本地项目文件夹到此窗口...");
        }
        InputType::Zip => {
            EnableWindow(ui.h_btn_browse, 1);
            set_cue(ui.h_edit_path, "点击右侧“浏览”或直接拖放本地 .zip 压缩包到此窗口...");
        }
    }
}

fn current_date_string() -> String {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
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

fn build_default_filename(input_type: InputType, input: &str, format: OutputFormat) -> String {
    let raw_name = match input_type {
        InputType::Url => {
            let trimmed = input.trim().trim_end_matches('/');
            let last = trimmed.rsplit('/').next().unwrap_or("codebase");
            last.strip_suffix(".git").unwrap_or(last).to_string()
        }
        InputType::Folder => Path::new(input)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "codebase".to_string()),
        InputType::Zip => Path::new(input)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "codebase".to_string()),
    };

    let base = sanitize_filename(&raw_name);
    let date = current_date_string();
    let ext = match format {
        OutputFormat::Xml => "xml",
        OutputFormat::Markdown => "md",
        OutputFormat::Plain => "txt",
    };

    format!("{base}_{date}.{ext}")
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    const IGNORED_DIRS: &[&str] = &[".git", "node_modules", "build", "dist", "target", ".dart_tool"];
    const IGNORED_EXTS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "zip", "tar", "gz", "7z", "rar",
        "exe", "dll", "so", "dylib", "pdb", "apk", "pdf",
    ];
    const IGNORED_FILES: &[&str] = &[".ds_store", "thumbs.db"];

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };

        let in_ignored_dir = rel.components().any(|comp| {
            if let Component::Normal(os_str) = comp {
                let s = os_str.to_string_lossy();
                IGNORED_DIRS.iter().any(|&d| s.eq_ignore_ascii_case(d))
            } else {
                false
            }
        });

        if in_ignored_dir {
            continue;
        }

        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            let name_lower = path
                .file_name()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if IGNORED_FILES.contains(&name_lower.as_str()) {
                continue;
            }

            let ext_lower = path
                .extension()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if IGNORED_EXTS.contains(&ext_lower.as_str()) {
                continue;
            }

            out.push(path);
        }
    }
    Ok(())
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

fn extract_zip(zip_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| format!("无法打开 ZIP 文件: {e}"))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| format!("解析 ZIP 压缩包失败: {e}"))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取 ZIP 条目失败: {e}"))?;
        let Some(rel_path) = entry.enclosed_name() else {
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

fn execute_pack(
    input_type: InputType,
    input: &str,
    output_path: &Path,
    format: OutputFormat,
    include_summary: bool,
    include_structure: bool,
) -> Result<usize, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let mut temp_dir: Option<PathBuf> = None;

    let result = (|| -> Result<usize, String> {
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
                temp_dir = Some(dir.clone());
                extract_zip(Path::new(input), &dir)?;
                dir
            }
            InputType::Url => {
                let dir = std::env::temp_dir().join(format!("repomix_git_{stamp}"));
                temp_dir = Some(dir.clone());
                let output = Command::new("git")
                    .args(["clone", "--depth", "1", input])
                    .arg(&dir)
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                    .map_err(|e| format!("无法运行 git，请确认本机已安装 Git: {e}"))?;

                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("Git Clone 失败: {}", err.trim()));
                }
                dir
            }
        };

        let mut files = Vec::new();
        collect_files(&target_dir, &target_dir, &mut files)
            .map_err(|e| format!("遍历目录失败: {e}"))?;

        let mut items: Vec<(String, PathBuf)> = files
            .into_iter()
            .filter_map(|p| {
                let rel = p.strip_prefix(&target_dir).ok()?;
                let norm = rel.to_string_lossy().replace('\\', "/");
                Some((norm, p))
            })
            .collect();

        items.sort_by(|a, b| a.0.cmp(&b.0));

        let mut out = String::with_capacity(64 * 1024);

        if include_structure {
            out.push_str("=== DIRECTORY STRUCTURE ===\n");
            for (rel, _) in &items {
                out.push_str("- ");
                out.push_str(rel);
                out.push('\n');
            }
            out.push_str("===========================\n\n");
        }

        if include_summary {
            out.push_str("=== FILE SUMMARY ===\n");
            for (rel, _) in &items {
                out.push_str("Path: ");
                out.push_str(rel);
                out.push('\n');
            }
            out.push_str("====================\n\n");
        }

        out.push_str("=== CODEBASE CONTENT ===\n\n");

        let mut packed_count = 0usize;
        for (rel, full_path) in &items {
            let Ok(text) = fs::read_to_string(full_path) else {
                continue;
            };
            packed_count += 1;

            match format {
                OutputFormat::Xml => {
                    out.push_str("<file path=\"");
                    out.push_str(rel);
                    out.push_str("\">\n");
                    out.push_str(&text);
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("</file>\n\n");
                }
                OutputFormat::Markdown => {
                    let ext = full_path
                        .extension()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let fence = choose_fence(&text);
                    out.push_str("### File: ");
                    out.push_str(rel);
                    out.push('\n');
                    out.push_str(&fence);
                    out.push_str(&ext);
                    out.push('\n');
                    out.push_str(&text);
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&fence);
                    out.push_str("\n\n");
                }
                OutputFormat::Plain => {
                    out.push_str("--- START: ");
                    out.push_str(rel);
                    out.push_str(" ---\n");
                    out.push_str(&text);
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("--- END ---\n\n");
                }
            }
        }

        fs::write(output_path, out.as_bytes()).map_err(|e| format!("保存输出文件失败: {e}"))?;
        Ok(packed_count)
    })();

    if let Some(dir) = temp_dir {
        let _ = fs::remove_dir_all(dir);
    }

    result
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
                "Repomix Portable (Rust Win32)",
                WS_CHILD | WS_VISIBLE,
                0,
                s(20),
                s(14),
                s(380),
                s(26),
                hwnd,
                0,
                h_inst,
                font_title,
            );
            create_ctrl(
                "STATIC",
                "支持直接拖放文件夹 / ZIP 压缩包，或输入 Git 仓库链接",
                WS_CHILD | WS_VISIBLE,
                0,
                s(20),
                s(42),
                s(520),
                s(18),
                hwnd,
                0,
                h_inst,
                font_normal,
            );

            create_ctrl(
                "BUTTON",
                "输入来源",
                WS_CHILD | WS_VISIBLE | BS_GROUPBOX,
                0,
                s(20),
                s(68),
                s(540),
                s(88),
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
                s(34),
                s(90),
                s(85),
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
                s(130),
                s(90),
                s(100),
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
                s(242),
                s(90),
                s(105),
                s(22),
                hwnd,
                ID_RADIO_ZIP,
                h_inst,
                font_normal,
            );

            let h_edit_path = create_ctrl(
                "EDIT",
                "",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL,
                WS_EX_CLIENTEDGE,
                s(34),
                s(118),
                s(426),
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
                s(468),
                s(117),
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
                s(20),
                s(166),
                s(255),
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
                s(34),
                s(194),
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
                s(102),
                s(194),
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
                s(200),
                s(194),
                s(60),
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
                s(288),
                s(166),
                s(272),
                s(68),
                hwnd,
                0,
                h_inst,
                font_normal,
            );

            let h_chk_summary = create_ctrl(
                "BUTTON",
                "包含文件摘要 (File Summary)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_GROUP | BS_AUTOCHECKBOX,
                0,
                s(302),
                s(187),
                s(240),
                s(20),
                hwnd,
                ID_CHK_SUMMARY,
                h_inst,
                font_normal,
            );
            let h_chk_structure = create_ctrl(
                "BUTTON",
                "包含目录结构 (Directory Structure)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                0,
                s(302),
                s(209),
                s(245),
                s(20),
                hwnd,
                ID_CHK_STRUCTURE,
                h_inst,
                font_normal,
            );

            let h_btn_pack = create_ctrl(
                "BUTTON",
                "开始打包 (Pack)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
                0,
                s(364),
                s(248),
                s(118),
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
                s(490),
                s(248),
                s(70),
                s(32),
                hwnd,
                ID_BTN_RESET,
                h_inst,
                font_normal,
            );

            let h_lbl_status = create_ctrl(
                "STATIC",
                "就绪。支持直接拖放文件夹或 .zip 压缩包。",
                WS_CHILD | WS_VISIBLE,
                0,
                s(20),
                s(255),
                s(334),
                s(24),
                hwnd,
                ID_LBL_STATUS,
                h_inst,
                font_normal,
            );

            set_checked(h_radio_dir, true);
            set_checked(h_radio_md, true);
            set_checked(h_chk_summary, true);
            set_checked(h_chk_structure, true);

            let state = UiState {
                h_radio_url,
                h_radio_dir,
                h_radio_zip,
                h_edit_path,
                h_btn_browse,
                h_radio_xml,
                h_radio_md,
                h_radio_plain,
                h_chk_summary,
                h_chk_structure,
                h_btn_pack,
                h_btn_reset,
                h_lbl_status,
                font_normal,
                font_title,
                bg_brush,
                is_processing: false,
            };
            refresh_input_ui(&state);

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
                    if let Some(ui) = cell.borrow().as_ref() {
                        if ui.is_processing {
                            return;
                        }
                        let p = Path::new(&path);
                        if p.is_dir() {
                            set_checked(ui.h_radio_url, false);
                            set_checked(ui.h_radio_dir, true);
                            set_checked(ui.h_radio_zip, false);
                            set_text(ui.h_edit_path, &path);
                            refresh_input_ui(ui);
                            set_text(ui.h_lbl_status, "已加载拖放的文件夹。");
                        } else if path.to_ascii_lowercase().ends_with(".zip") {
                            set_checked(ui.h_radio_url, false);
                            set_checked(ui.h_radio_dir, false);
                            set_checked(ui.h_radio_zip, true);
                            set_text(ui.h_edit_path, &path);
                            refresh_input_ui(ui);
                            set_text(ui.h_lbl_status, "已加载拖放的 ZIP 压缩包。");
                        } else {
                            set_text(ui.h_lbl_status, "仅支持拖放文件夹或 .zip 压缩包。");
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

            if notify == BN_CLICKED {
                match ctrl_id {
                    ID_RADIO_URL | ID_RADIO_DIR | ID_RADIO_ZIP => {
                        UI.with(|cell| {
                            if let Some(ui) = cell.borrow().as_ref() {
                                refresh_input_ui(ui);
                            }
                        });
                    }
                    ID_BTN_BROWSE => {
                        let input_type = UI.with(|cell| {
                            cell.borrow().as_ref().map(|ui| get_input_type(ui))
                        });
                        if let Some(t) = input_type {
                            let picked = match t {
                                InputType::Folder => {
                                    FileDialog::new().set_title("选择项目文件夹").pick_folder()
                                }
                                InputType::Zip => FileDialog::new()
                                    .set_title("选择项目 ZIP 压缩包")
                                    .add_filter("ZIP 压缩包", &["zip"])
                                    .pick_file(),
                                InputType::Url => None,
                            };
                            if let Some(path) = picked {
                                UI.with(|cell| {
                                    if let Some(ui) = cell.borrow().as_ref() {
                                        set_text(ui.h_edit_path, &path.to_string_lossy());
                                    }
                                });
                            }
                        }
                    }
                    ID_BTN_RESET => {
                        UI.with(|cell| {
                            if let Some(ui) = cell.borrow().as_ref() {
                                if !ui.is_processing {
                                    set_text(ui.h_edit_path, "");
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
                                set_text(ui.h_lbl_status, "请先选择文件夹、ZIP 或输入 Git 链接。");
                                return None;
                            }
                            let input_type = get_input_type(ui);
                            let format = get_output_format(ui);
                            let include_summary = is_checked(ui.h_chk_summary);
                            let include_structure = is_checked(ui.h_chk_structure);
                            Some((input_type, input, format, include_summary, include_structure))
                        });

                        if let Some((input_type, input, format, inc_sum, inc_str)) = params {
                            let default_name = build_default_filename(input_type, &input, format);
                            let (filter_label, ext) = match format {
                                OutputFormat::Xml => ("XML 文件", "xml"),
                                OutputFormat::Markdown => ("Markdown 文件", "md"),
                                OutputFormat::Plain => ("纯文本文件", "txt"),
                            };

                            if let Some(mut save_path) = FileDialog::new()
                                .set_title("保存打包结果")
                                .set_file_name(&default_name)
                                .add_filter(filter_label, &[ext])
                                .save_file()
                            {
                                if save_path.extension().is_none() {
                                    save_path.set_extension(ext);
                                }

                                UI.with(|cell| {
                                    if let Some(ui) = cell.borrow_mut().as_mut() {
                                        ui.is_processing = true;
                                        EnableWindow(ui.h_btn_pack, 0);
                                        EnableWindow(ui.h_btn_reset, 0);
                                        set_text(ui.h_lbl_status, "正在打包中，请稍候...");
                                    }
                                });

                                let hwnd_val = hwnd as usize;
                                thread::spawn(move || {
                                    let res = execute_pack(
                                        input_type,
                                        &input,
                                        &save_path,
                                        format,
                                        inc_sum,
                                        inc_str,
                                    );
                                    let msg_tuple = match res {
                                        Ok(count) => {
                                            let fname = save_path
                                                .file_name()
                                                .map(|s| s.to_string_lossy().into_owned())
                                                .unwrap_or_default();
                                            (true, format!("打包成功（共 {count} 个文件）：{fname}"))
                                        }
                                        Err(e) => (false, format!("错误：{e}")),
                                    };

                                    let lock = PACK_RESULT.get_or_init(|| Mutex::new(None));
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
            let msg_text = PACK_RESULT
                .get()
                .and_then(|m| m.lock().ok())
                .and_then(|mut s| s.take())
                .map(|(_, text)| text)
                .unwrap_or_else(|| "处理完成。".to_string());

            UI.with(|cell| {
                if let Some(ui) = cell.borrow_mut().as_mut() {
                    ui.is_processing = false;
                    EnableWindow(ui.h_btn_pack, 1);
                    EnableWindow(ui.h_btn_reset, 1);
                    set_text(ui.h_lbl_status, &msg_text);
                }
            });
            0
        }

        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }

        WM_DESTROY => {
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

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: h_inst,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: bg,
            lpszMenuName: null_mut(),
            lpszClassName: class_name.as_ptr(),
        };

        RegisterClassW(&wc);

        let hdc = GetDC(null_mut());
        let dpi = if !hdc.is_null() {
            let d = GetDeviceCaps(hdc, LOGPIXELSX);
            ReleaseDC(null_mut(), hdc);
            if d > 0 { d } else { 96 }
        } else {
            96
        };

        let win_w = scale(596, dpi);
        let win_h = scale(335, dpi);
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let pos_x = (screen_w - win_w) / 2;
        let pos_y = (screen_h - win_h) / 2;

        let title = to_wide("Repomix Portable");
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
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
            DeleteObject(bg);
            return;
        }

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        DeleteObject(bg);
    }
}
