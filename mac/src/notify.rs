//! macOS 系统通知。
//!
//! 装成 .app 跑的时候走 UserNotifications（UNUserNotificationCenter）：通知以 AAA 自己的
//! 名义发出，点一下就回到 App 并打开对应会话（`userInfo.session`）。以前 shell out
//! `osascript display notification`，发件人是「脚本编辑器」，点了只会打开脚本编辑器。
//! `cargo run` 这种没有 bundle 的场合 UN* 会直接崩，退回 osascript。
//!
//! 点击 → 会话：delegate 收到响应后把动作丢进一个全局通道，RootView 那头
//! 用 gpui 任务收，激活窗口并 open_session。
//!
//! v1.27：待回复且是**权限请求**的那条通知，横幅上直接给「允许 / 拒绝」
//! （`UNNotificationCategory` + 两个 `UNNotificationAction`）——按了就不回 App，
//! 直接 `POST /sessions/:id/permission`。不然为了放行一句 `cargo test`，
//! 要点通知、等 App 起来、翻到那条会话、滚到底。

use std::sync::{Mutex, OnceLock};

use futures::channel::mpsc;

/// 用户在通知上做了什么
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyAction {
    /// 点了通知本体：回到 App 打开这条会话
    Open(String),
    /// 按了横幅上的「允许 / 拒绝」：不回 App，直接替答
    Decide(String, &'static str),
}

/// 通知上的动作
static CLICKS: OnceLock<Mutex<Option<mpsc::UnboundedSender<NotifyAction>>>> = OnceLock::new();

/// 生成 AppleScript 片段（纯函数，便于测试转义）——无 bundle 时的回落路径
pub fn build_script(title: &str, body: &str) -> String {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "display notification \"{}\" with title \"{}\"",
        esc(body),
        esc(title)
    )
}

fn send_osascript(title: &str, body: &str) {
    let script = build_script(title, body);
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// 是否在 .app bundle 里运行（UN* 只在这种情况下能用）
fn bundled() -> bool {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

/// App 启动时调一次：登记点击回调的收件通道、设 delegate、申请通知权限。
/// 返回点击流（会话 id），由调用方在 gpui 任务里消费。
pub fn install() -> mpsc::UnboundedReceiver<NotifyAction> {
    let (tx, rx) = mpsc::unbounded();
    let _ = CLICKS.set(Mutex::new(Some(tx)));
    if bundled() {
        native::install();
    }
    rx
}

/// 发送通知（不阻塞 UI；失败静默）。`session` = 点击后要打开的会话 id；
/// `decidable` = 这条是能替答的权限请求，横幅上多给两个按钮。
pub fn send(title: &str, body: &str, session: &str, decidable: bool) {
    if bundled() {
        native::send(title, body, session, decidable);
    } else {
        send_osascript(title, body);
    }
}

fn dispatch(action: NotifyAction) {
    if let Some(m) = CLICKS.get() {
        if let Some(tx) = m.lock().unwrap().as_ref() {
            let _ = tx.unbounded_send(action);
        }
    }
}

mod native {
    use std::sync::OnceLock;

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{define_class, MainThreadOnly};
    use objc2_foundation::{NSArray, NSDictionary, NSObject, NSObjectProtocol, NSString};
    use objc2_foundation::NSSet;
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
        UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    const KEY_SESSION: &str = "session";
    /// 带「允许 / 拒绝」的那一类通知
    const CATEGORY_PERM: &str = "AAA_PERM";
    const ACTION_ALLOW: &str = "AAA_ALLOW";
    const ACTION_DENY: &str = "AAA_DENY";

    define_class!(
        // SAFETY: NSObject 没有子类化要求；本类不实现 Drop
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "AAANotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            // App 在前台时也把横幅摆出来（不然前台收到的通知被系统吞掉）
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
            }

            // 点了通知：把会话 id 交回 Rust 那头
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion: &block2::DynBlock<dyn Fn()>,
            ) {
                let info = response.notification().request().content().userInfo();
                let key = NSString::from_str(KEY_SESSION);
                if let Some(v) = info.objectForKey(&key) {
                    if let Ok(s) = v.downcast::<NSString>() {
                        let id = s.to_string();
                        super::dispatch(match response.actionIdentifier().to_string().as_str() {
                            ACTION_ALLOW => super::NotifyAction::Decide(id, "allow"),
                            ACTION_DENY => super::NotifyAction::Decide(id, "deny"),
                            _ => super::NotifyAction::Open(id),
                        });
                    }
                }
                completion.call(());
            }
        }
    );

    // delegate 是弱引用：必须自己持有一份，否则一发完就没了
    static DELEGATE: OnceLock<DelegateHolder> = OnceLock::new();
    struct DelegateHolder(#[allow(dead_code)] Retained<Delegate>);
    // SAFETY: 只在主线程创建与使用（UN* 的回调也在主线程）；OnceLock 仅为延长生命周期
    unsafe impl Send for DelegateHolder {}
    unsafe impl Sync for DelegateHolder {}

    pub fn install() {
        let Some(mtm) = objc2::MainThreadMarker::new() else { return };
        let delegate: Retained<Delegate> = unsafe { objc2::msg_send![Delegate::alloc(mtm), init] };
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let _ = DELEGATE.set(DelegateHolder(delegate));
        // 「允许 / 拒绝」那一类：注册一次，之后按 categoryIdentifier 挂上去
        let action = |id: &str, title: &str, destructive: bool| {
            let opts = if destructive {
                UNNotificationActionOptions::Destructive
            } else {
                UNNotificationActionOptions::empty()
            };
            unsafe {
                UNNotificationAction::actionWithIdentifier_title_options(
                    &NSString::from_str(id),
                    &NSString::from_str(title),
                    opts,
                )
            }
        };
        let allow = action(ACTION_ALLOW, "允许", false);
        let deny = action(ACTION_DENY, "拒绝", true);
        let actions = NSArray::from_retained_slice(&[allow, deny]);
        let category = unsafe {
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(CATEGORY_PERM),
                &actions,
                &NSArray::from_retained_slice(&[]),
                UNNotificationCategoryOptions::empty(),
            )
        };
        center.setNotificationCategories(&NSSet::from_retained_slice(&[category]));
        let done = block2::RcBlock::new(|_granted: objc2::runtime::Bool, _err: *mut objc2_foundation::NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &done,
        );
    }

    pub fn send(title: &str, body: &str, session: &str, decidable: bool) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        if decidable {
            content.setCategoryIdentifier(&NSString::from_str(CATEGORY_PERM));
        }
        let info: Retained<NSDictionary<NSString, NSString>> =
            NSDictionary::from_slices(&[&*NSString::from_str(KEY_SESSION)], &[&*NSString::from_str(session)]);
        // SAFETY: 键值都是 NSString，是 plist 合法类型；泛型擦成 AnyObject 只是类型层面的转换
        let info: Retained<NSDictionary> = unsafe { Retained::cast_unchecked(info) };
        unsafe { content.setUserInfo(&info) };
        let id = NSString::from_str(&format!("aaa-{}-{}", session, chrono::Utc::now().timestamp_millis()));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
        UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_shape() {
        assert_eq!(
            build_script("会话 A", "等待输入"),
            r#"display notification "等待输入" with title "会话 A""#
        );
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        let s = build_script(r#"a"b"#, r#"c\d "e""#);
        assert_eq!(
            s,
            r#"display notification "c\\d \"e\"" with title "a\"b""#
        );
    }
}
