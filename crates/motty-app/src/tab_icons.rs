//! The icons tab rules pick from: Phosphor's duotone set (MIT, see
//! `assets/tab-icons/LICENSE`), under names that follow Otty's.

macro_rules! tab_icons {
    ($($name:literal),* $(,)?) => {
        /// Every icon name, in the picker's order.
        pub const NAMES: &[&str] = &[$($name),*];

        /// An icon's SVG.
        pub fn svg(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!("../assets/tab-icons/", $name, ".svg")).as_slice()),)*
                _ => None,
            }
        }
    };
}

tab_icons!(
    "terminal",
    "server",
    "database",
    "cloud",
    "globe",
    "folder",
    "code",
    "file",
    "markdown",
    "git-branch",
    "git-commit",
    "github",
    "gitlab",
    "docker",
    "claude",
    "openai",
    "robot",
    "bug",
    "rocket",
    "flask",
    "package",
    "cube",
    "gear",
    "wrench",
    "hammer",
    "lightning",
    "fire",
    "star",
    "heart",
    "flag",
    "bookmark",
    "book",
    "notebook",
    "pencil",
    "lock",
    "key",
    "shield",
    "user",
    "users",
    "house",
    "briefcase",
    "chart",
    "pulse",
    "clock",
    "calendar",
    "bell",
    "chat",
    "envelope",
    "music",
    "image",
    "desktop",
    "laptop",
    "cpu",
    "apple",
    "linux",
    "windows",
    "android",
    "python",
    "javascript",
    "typescript",
    "rust",
    "html",
    "css",
    "coffee",
    "leaf",
    "planet",
    "moon",
    "sun",
    "network",
    "plug",
    "kanban",
    "sparkle",
);

/// The asset path gpui loads an icon from.
pub fn path(name: &str) -> Option<String> {
    svg(name).map(|_| format!("tab-icons/{name}.svg"))
}
