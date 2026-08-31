//! macOS TCC permission probing / requesting, in-process FFI so the TCC
//! attribution lands on the daemon binary (agent subprocesses inherit it).
//!
//! Status probing is strictly read-only (no prompts). `request` fires the
//! system dialogs (or opens System Settings) and returns immediately; the
//! actual user interaction happens on the Mac's screen.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct PermStatus {
    pub id: &'static str,
    pub label: &'static str,
    pub status: &'static str, // granted|denied|undetermined|unknown|needs_settings
}

pub const PERM_IDS: &[(&str, &str)] = &[
    ("accessibility", "辅助功能"),
    ("screen_recording", "屏幕录制"),
    ("input_monitoring", "输入监控"),
    ("full_disk_access", "完全磁盘访问"),
    ("automation_system_events", "自动化 · System Events"),
    ("automation_finder", "自动化 · Finder"),
    ("camera", "摄像头"),
    ("microphone", "麦克风"),
];

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::c_void;
    use std::os::raw::c_char;

    pub const UTF8: u32 = 0x0800_0100;

    #[repr(C)]
    pub struct AEDesc {
        pub descriptor_type: u32,
        pub data_handle: *mut c_void,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        pub fn AXIsProcessTrusted() -> bool;
        pub fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        pub static kAXTrustedCheckOptionPrompt: *const c_void; // CFStringRef
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub fn CGPreflightScreenCaptureAccess() -> bool;
        pub fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        // request type 1 = kIOHIDRequestTypeListenEvent
        pub fn IOHIDCheckAccess(request_type: u32) -> u32;
        pub fn IOHIDRequestAccess(request_type: u32) -> bool;
    }

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        pub fn AECreateDesc(
            type_code: u32,
            data_ptr: *const c_void,
            data_size: isize,
            result: *mut AEDesc,
        ) -> i16;
        pub fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
        pub fn AEDeterminePermissionToAutomateTarget(
            target: *const AEDesc,
            event_class: u32,
            event_id: u32,
            ask_user_if_needed: u8,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub static kCFBooleanTrue: *const c_void;
        pub static kCFTypeDictionaryKeyCallBacks: u8;
        pub static kCFTypeDictionaryValueCallBacks: u8;
        pub fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const u8,
            value_callbacks: *const u8,
        ) -> *const c_void;
        pub fn CFStringCreateWithCString(
            alloc: *const c_void,
            cstr: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        pub fn CFRelease(cf: *const c_void);
    }

    // objc runtime for AVCaptureDevice (avoids the objc2 crate stack)
    #[link(name = "objc")]
    extern "C" {
        pub fn objc_getClass(name: *const c_char) -> *mut c_void;
        pub fn sel_registerName(name: *const c_char) -> *mut c_void;
        pub fn objc_msgSend();
    }
    // force-load AVFoundation so the class is registered
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}

    pub fn fourcc(s: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*s)
    }

    /// AVCaptureDevice.authorizationStatus(for:) — read-only, no prompt.
    /// media: b"vide" -> camera, b"soun" -> microphone.
    /// Returns None if the class/selector can't be resolved.
    pub fn av_authorization_status(media: &str) -> Option<i64> {
        unsafe {
            let cls = objc_getClass(c"AVCaptureDevice".as_ptr());
            if cls.is_null() {
                return None;
            }
            let sel = sel_registerName(c"authorizationStatusForMediaType:".as_ptr());
            if sel.is_null() {
                return None;
            }
            let cmedia = std::ffi::CString::new(media).ok()?;
            let cfstr = CFStringCreateWithCString(std::ptr::null(), cmedia.as_ptr(), UTF8);
            if cfstr.is_null() {
                return None;
            }
            // CFStringRef is toll-free bridged to NSString*
            let f: extern "C" fn(*mut c_void, *mut c_void, *const c_void) -> i64 =
                std::mem::transmute(objc_msgSend as *const c_void);
            let status = f(cls, sel, cfstr);
            CFRelease(cfstr);
            Some(status)
        }
    }

    /// AEDeterminePermissionToAutomateTarget on a bundle-id address desc.
    pub fn ae_determine(bundle_id: &str, ask: bool) -> i32 {
        unsafe {
            let mut desc = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
            let err = AECreateDesc(
                fourcc(b"bund"), // typeApplicationBundleID
                bundle_id.as_ptr() as *const c_void,
                bundle_id.len() as isize,
                &mut desc,
            );
            if err != 0 {
                return err as i32;
            }
            let wild = fourcc(b"****"); // typeWildCard
            let status =
                AEDeterminePermissionToAutomateTarget(&desc, wild, wild, ask as u8);
            AEDisposeDesc(&mut desc);
            status
        }
    }

    /// AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true}).
    pub fn ax_prompt() -> bool {
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let vals = [kCFBooleanTrue];
            let dict = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                vals.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let r = AXIsProcessTrustedWithOptions(dict);
            if !dict.is_null() {
                CFRelease(dict);
            }
            r
        }
    }

    pub fn open_settings(pane: &str) {
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
        let _ = std::process::Command::new("open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

fn home() -> std::path::PathBuf {
    std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn full_disk_status() -> &'static str {
    let tcc = home()
        .join("Library")
        .join("Application Support")
        .join("com.apple.TCC")
        .join("TCC.db");
    match std::fs::File::open(&tcc) {
        Ok(_) => "granted",
        Err(_) => "needs_settings",
    }
}

#[cfg(target_os = "macos")]
fn status_one(id: &str) -> &'static str {
    match id {
        "accessibility" => {
            if unsafe { ffi::AXIsProcessTrusted() } {
                "granted"
            } else {
                "undetermined" // API can't distinguish denied vs never-asked
            }
        }
        "screen_recording" => {
            if unsafe { ffi::CGPreflightScreenCaptureAccess() } {
                "granted"
            } else {
                "undetermined"
            }
        }
        "input_monitoring" => match unsafe { ffi::IOHIDCheckAccess(1) } {
            0 => "granted",
            1 => "denied",
            _ => "undetermined",
        },
        "full_disk_access" => full_disk_status(),
        "automation_system_events" | "automation_finder" => {
            let bundle = if id == "automation_finder" {
                "com.apple.finder"
            } else {
                "com.apple.systemevents"
            };
            match ffi::ae_determine(bundle, false) {
                0 => "granted",
                -1743 => "denied",        // errAEEventNotPermitted
                -1744 => "undetermined",  // errAEEventWouldRequireUserConsent
                -600 => "undetermined",   // procNotFound (target not running)
                _ => "unknown",
            }
        }
        "camera" | "microphone" => {
            let media = if id == "camera" { "vide" } else { "soun" };
            match ffi::av_authorization_status(media) {
                Some(3) => "granted",
                Some(2) => "denied",
                Some(1) => "denied", // restricted
                Some(0) => "undetermined",
                _ => "unknown",
            }
        }
        _ => "unknown",
    }
}

#[cfg(not(target_os = "macos"))]
fn status_one(_id: &str) -> &'static str {
    "unknown"
}

pub fn status_all() -> Vec<PermStatus> {
    PERM_IDS
        .iter()
        .map(|(id, label)| PermStatus { id, label, status: status_one(id) })
        .collect()
}

/// Fire the permission requests. Returns (triggered, opened_settings).
/// All prompting work runs on detached threads: callers get an immediate
/// answer (202 semantics) and the dialogs appear on the Mac's screen.
pub fn request(ids: &[String]) -> (Vec<&'static str>, Vec<&'static str>) {
    let all = ids.iter().any(|s| s == "all");
    let want = |id: &str| all || ids.iter().any(|s| s == id);
    let mut triggered = Vec::new();
    let mut opened = Vec::new();

    #[cfg(target_os = "macos")]
    {
        if want("accessibility") {
            std::thread::spawn(|| {
                let _ = ffi::ax_prompt();
            });
            triggered.push("accessibility");
        }
        if want("screen_recording") {
            std::thread::spawn(|| unsafe {
                let _ = ffi::CGRequestScreenCaptureAccess();
            });
            triggered.push("screen_recording");
        }
        if want("input_monitoring") {
            std::thread::spawn(|| unsafe {
                let _ = ffi::IOHIDRequestAccess(1);
            });
            triggered.push("input_monitoring");
        }
        if want("automation_system_events") {
            std::thread::spawn(|| {
                let _ = ffi::ae_determine("com.apple.systemevents", true);
            });
            triggered.push("automation_system_events");
        }
        if want("automation_finder") {
            std::thread::spawn(|| {
                let _ = ffi::ae_determine("com.apple.finder", true);
            });
            triggered.push("automation_finder");
        }
        // Camera/mic: AVCaptureDevice requestAccess would abort in a process
        // without a usage-description Info.plist, so route to Settings.
        if want("camera") {
            ffi::open_settings("Privacy_Camera");
            opened.push("camera");
        }
        if want("microphone") {
            ffi::open_settings("Privacy_Microphone");
            opened.push("microphone");
        }
        if want("full_disk_access") {
            ffi::open_settings("Privacy_AllFiles");
            opened.push("full_disk_access");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = want;
    }
    (triggered, opened)
}

/// CLI: `aaa-daemon perms status`.
pub fn cli_status() {
    for p in status_all() {
        println!("{:26} {:14} {}", p.id, p.status, p.label);
    }
}

/// CLI: `aaa-daemon perms request-all`.
pub fn cli_request_all() {
    let (triggered, opened) = request(&["all".to_string()]);
    println!("triggered: {}", triggered.join(", "));
    println!("opened_settings: {}", opened.join(", "));
    println!("(弹窗在 Mac 屏幕上, 请到 Mac 前完成授权)");
    // give the detached prompt threads a moment to fire before exit
    std::thread::sleep(std::time::Duration::from_millis(1500));
}
