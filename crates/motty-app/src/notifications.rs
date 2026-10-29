//! Desktop notifications (OSC 9 / OSC 777) through the user notification
//! center.

/// Shows a notification. Does nothing outside an app bundle, where the
/// notification center raises an exception.
#[cfg(target_os = "macos")]
pub fn notify(title: &str, body: &str) {
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
    let id = NSString::from_str(&format!(
        "motty-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
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
        UNNotification, UNNotificationPresentationOptions, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    define_class!(
        /// Presents banners even while Motty is the active app.
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
        }
    );
}

#[cfg(not(target_os = "macos"))]
pub fn notify(title: &str, body: &str) {
    tracing::debug!(title, body, "notification");
}
