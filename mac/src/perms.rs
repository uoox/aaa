//! `AAA.app --request-permissions`：把归责到本 App 的 TCC 弹窗一次触发完。
//!
//! daemon 的权限（辅助功能/屏幕录制/FDA…）归责到 daemon 进程，由
//! `aaa-daemon perms` / CLI 菜单负责；这里只管 **App 自己** 的主体：
//! 辅助功能 + 屏幕录制立即弹窗，完全磁盘访问无法弹窗、打开对应设置面板，
//! 最后发一条测试通知（osascript 通道，顺带验证通知可达）。
//! ad-hoc 签名意味着重新构建后这些授权会失效，需要重新点一遍。

use std::ffi::c_void;

// ApplicationServices 是伞框架，把 CoreGraphics / CoreFoundation / HIServices
// 一并带进来，链接这一个就够了
#[link(name = "ApplicationServices", kind = "framework")]
#[allow(non_snake_case)]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num: isize,
        key_cb: *const c_void,
        value_cb: *const c_void,
    ) -> *const c_void;
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        c_str: *const std::ffi::c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFRelease(cf: *const c_void);
    static kCFBooleanTrue: *const c_void;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

const UTF8: u32 = 0x0800_0100;

fn ax_prompt() {
    unsafe {
        let key = CFStringCreateWithCString(
            std::ptr::null(),
            c"AXTrustedCheckOptionPrompt".as_ptr(),
            UTF8,
        );
        let keys = [key];
        let vals = [kCFBooleanTrue];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            vals.as_ptr(),
            1,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        );
        AXIsProcessTrustedWithOptions(dict);
        if !dict.is_null() {
            CFRelease(dict);
        }
        CFRelease(key);
    }
}

/// 触发全部可弹窗项 + 打开 FDA 面板。在窗口起来之后调用（TCC 弹窗
/// 需要一个前台 App 主体挂靠）。
pub fn request_all() {
    ax_prompt();
    unsafe {
        CGRequestScreenCaptureAccess();
    }
    // FDA 没有 API 弹窗，只能开面板让人把 AAA.app 拖进去
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .spawn();
    crate::notify::send("AAA", "权限申请已触发：请在弹窗与设置面板中逐个允许");
}

pub fn requested_via_args() -> bool {
    std::env::args().any(|a| a == "--request-permissions")
}
