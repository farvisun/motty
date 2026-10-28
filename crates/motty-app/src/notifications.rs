//! Desktop notifications (OSC 9 / OSC 777) through the user notification
//! center.

/// Shows a notification. Does nothing outside an app bundle, where the
/// notification center raises an exception.
#[cfg(target_os = "macos")]
pub fn notify(title: &str, body: &str) {
    use std::sync::Once;

    use block2::RcBlock;
    use objc2::runtime::Bool;
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

#[cfg(not(target_os = "macos"))]
pub fn notify(title: &str, body: &str) {
    tracing::debug!(title, body, "notification");
}
