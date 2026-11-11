//! Desktop notifications (OSC 9 / OSC 777) through the user notification
//! center. A notification about a pane carries its id; clicking it goes to
//! that pane, its tab and window.

use std::sync::OnceLock;

use gpui::App;

/// Prefix of a notification's identifier; a pane's id follows, then a
/// stamp that keeps identifiers unique.
const ID_PREFIX: &str = "motty-";

/// Clicked notifications' panes, from the notification center's queue to
/// the main thread.
static CLICKS: OnceLock<async_channel::Sender<u64>> = OnceLock::new();

/// Starts handling clicks on notifications: each goes to its pane.
pub fn install(cx: &mut App) {
    let (tx, rx) = async_channel::unbounded::<u64>();
    if CLICKS.set(tx).is_err() {
        return;
    }
    cx.spawn(async move |cx| {
        while let Ok(pane) = rx.recv().await {
            cx.update(|cx| crate::ipc::reveal_pane_by_id(pane, cx));
        }
    })
    .detach();
}

/// The notification identifier for `pane` (or none), unique per call.
fn identifier(pane: Option<u64>) -> String {
    // The clock alone can repeat (microseconds on macOS), so a counter
    // follows it.
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let stamp = format!("{time}.{count}");
    match pane {
        Some(pane) => format!("{ID_PREFIX}{pane}-{stamp}"),
        None => format!("{ID_PREFIX}{stamp}"),
    }
}

/// The pane in an identifier from [`identifier`].
fn pane_of(identifier: &str) -> Option<u64> {
    let (pane, _stamp) = identifier.strip_prefix(ID_PREFIX)?.split_once('-')?;
    pane.parse().ok()
}

/// A notification was clicked: go to its pane, if it names one.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn clicked(identifier: &str) {
    if let (Some(pane), Some(tx)) = (pane_of(identifier), CLICKS.get()) {
        let _ = tx.try_send(pane);
    }
}

/// Shows a notification; clicking it goes to `pane` when given. Does
/// nothing outside an app bundle, where the notification center raises an
/// exception.
#[cfg(target_os = "macos")]
pub fn notify(title: &str, body: &str, pane: Option<u64>) {
    use std::sync::Once;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::{AnyThread, msg_send};
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
        UNUserNotificationCenter,
    };

    static AUTHORIZE: Once = Once::new();

    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        tracing::debug!(title, body, "notification (no app bundle)");
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    AUTHORIZE.call_once(|| {
        // Banners show while Motty is in front too; `notification-foreground`
        // already decided whether to post.
        let delegate: Retained<mac::Delegate> = unsafe { msg_send![mac::Delegate::alloc(), init] };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // The center holds its delegate weakly; this one lives as long as
        // the app.
        std::mem::forget(delegate);
        let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
            // SAFETY: the error is null or valid for the call.
            match unsafe { error.as_ref() } {
                Some(e) => {
                    tracing::warn!("notifications not allowed: {}", e.localizedDescription())
                }
                None => tracing::debug!(granted = granted.as_bool(), "notification authorization"),
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &done,
        );
    });
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let id = NSString::from_str(&identifier(pane));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
    center.addNotificationRequest_withCompletionHandler(&request, None);
}

/// Bounces the Dock icon once, for a banner while Motty is in the back.
#[cfg(target_os = "macos")]
pub fn bounce_dock() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm)
            .requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn bounce_dock() {}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::runtime::{NSObject, NSObjectProtocol};
    use objc2::{AnyThread, define_class};
    use objc2_user_notifications::{
        UNNotification, UNNotificationPresentationOptions, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    define_class!(
        /// Presents banners even while Motty is the active app, and goes to
        /// a notification's pane when it's clicked.
        #[unsafe(super(NSObject))]
        #[thread_kind = AnyThread]
        #[name = "MottyNotificationDelegate"]
        pub struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                handler: &block2::DynBlock<dyn Fn()>,
            ) {
                let id = response.notification().request().identifier();
                super::clicked(&id.to_string());
                handler.call(());
            }
        }
    );
}

#[cfg(not(target_os = "macos"))]
pub fn notify(title: &str, body: &str, pane: Option<u64>) {
    tracing::debug!(title, body, ?pane, "notification");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_carries_the_pane() {
        assert_eq!(pane_of(&identifier(Some(42))), Some(42));
        assert_eq!(pane_of(&identifier(None)), None);
        assert_eq!(pane_of("motty-x-1"), None);
        assert_eq!(pane_of("other-7-1"), None);
    }

    #[test]
    fn identifiers_are_unique() {
        assert_ne!(identifier(Some(1)), identifier(Some(1)));
    }
}
