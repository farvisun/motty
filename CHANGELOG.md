# Changelog

All notable changes to Motty are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.0] - 2026-10-30

### Features

- **app:** check for updates once a day and from Motty › Check for Updates…; Install and Restart downloads the new Motty.dmg, checks its SHA-256, replaces the app and reopens it. Turn the daily check off with `check-for-updates` (Settings › General) ([4235b4a](https://github.com/farvisun/motty/commit/4235b4a))

### CI

- publish SHA256SUMS.txt with each release ([c35ad26](https://github.com/farvisun/motty/commit/c35ad26))

## [0.2.0] - 2026-10-30

### Features

- **term:** jump between prompts with ⌘↑ and ⌘↓ ([44e988d](https://github.com/farvisun/motty/commit/44e988d))
- **term:** kitty clipboard protocol (OSC 5522) and paste events (mode 5522): programs read and write pictures and HTML, not just text ([351307f](https://github.com/farvisun/motty/commit/351307f))

### Performance

- **term:** read the pty on its own thread, so programs keep writing while output is parsed ([8dc705e](https://github.com/farvisun/motty/commit/8dc705e))

### Refactoring

- **term:** move prompt lookup into the shell state ([087b65c](https://github.com/farvisun/motty/commit/087b65c))

### Documentation

- **site:** list the clipboard protocol ([f42e4da](https://github.com/farvisun/motty/commit/f42e4da))

## [0.1.1] - 2026-10-30

### Bug Fixes

- **term:** forward ⌘ chords the config unbinds to the program, as Super in the kitty keyboard protocol ([fa2e4ea](https://github.com/farvisun/motty/commit/fa2e4ea))
- **app:** keep titlebar buttons while hovering a resize edge ([3344d2f](https://github.com/farvisun/motty/commit/3344d2f))
- **settings:** stop listing installed fonts every frame; hover on sidebar items ([4e36a2f](https://github.com/farvisun/motty/commit/4e36a2f))

## [0.1.0] - 2026-10-29

First public release.

### Features

- **theme:** parse .ottytheme files and otty-compatible config.toml ([6cd065e](https://github.com/farvisun/motty/commit/6cd065e))
- **term:** pty session with key, mouse and paste encoding ([f7c8648](https://github.com/farvisun/motty/commit/f7c8648))
- **render:** gpui terminal element ([0a28b56](https://github.com/farvisun/motty/commit/0a28b56))
- **app:** single-pane terminal window with themed chrome ([5b7dca4](https://github.com/farvisun/motty/commit/5b7dca4))
- **term:** expose foreground process name and cwd ([0d430f2](https://github.com/farvisun/motty/commit/0d430f2))
- **xtask:** pixel parity harness against otty ([0a2b8ef](https://github.com/farvisun/motty/commit/0a2b8ef))
- **app:** sidebar tabs, title band and window bounds flag ([ceda506](https://github.com/farvisun/motty/commit/ceda506))
- **term:** name the shell behind login and detect running commands ([b2870c5](https://github.com/farvisun/motty/commit/b2870c5))
- **app:** match otty's multi-tab sidebar ([c3354c5](https://github.com/farvisun/motty/commit/c3354c5))
- **xtask:** app bundle and focused, multi-tab parity captures ([9a00485](https://github.com/farvisun/motty/commit/9a00485))
- **theme:** read otty enum config keys and config.local.toml ([2dcfe70](https://github.com/farvisun/motty/commit/2dcfe70))
- **app:** per-region native materials and a --theme flag ([c7e93cc](https://github.com/farvisun/motty/commit/c7e93cc))
- **xtask:** parity under a temporary otty theme, re-signed bundles ([1a7a925](https://github.com/farvisun/motty/commit/1a7a925))
- **app:** calibrate window materials against otty ([c2927dd](https://github.com/farvisun/motty/commit/c2927dd))
- **xtask:** solid backdrop captures for material themes ([336dc72](https://github.com/farvisun/motty/commit/336dc72))
- **app:** traffic lights follow the theme's appearance ([5d76943](https://github.com/farvisun/motty/commit/5d76943))
- **app:** bar tab indicator; plain sidebars in material windows ([d05958f](https://github.com/farvisun/motty/commit/d05958f))
- **term:** cursor-style, blink policy and cursor colors ([90b344e](https://github.com/farvisun/motty/commit/90b344e))
- **app:** titlebar buttons and a collapsible sidebar ([569dd6e](https://github.com/farvisun/motty/commit/569dd6e))
- **app:** reload config and themes live ([2ca3ef3](https://github.com/farvisun/motty/commit/2ca3ef3))
- **app:** split panes ([c6aaccb](https://github.com/farvisun/motty/commit/c6aaccb))
- **term:** own PTY loop with grapheme clustering ([191108a](https://github.com/farvisun/motty/commit/191108a))
- **xtask:** background tabs and a capture delay ([323b9b6](https://github.com/farvisun/motty/commit/323b9b6))
- **app:** text editing, scrolling and window keys like Otty ([c42be9c](https://github.com/farvisun/motty/commit/c42be9c))
- **term:** shell integration (OSC 133, OSC 7, notifications) ([655276b](https://github.com/farvisun/motty/commit/655276b))
- control socket and the motty CLI ([01ceb06](https://github.com/farvisun/motty/commit/01ceb06))
- **cli:** motty mcp, an MCP server for coding agents ([342a31b](https://github.com/farvisun/motty/commit/342a31b))
- **app:** desktop notifications from OSC 9/777 ([600a601](https://github.com/farvisun/motty/commit/600a601))
- find in scrollback ([5bcf172](https://github.com/farvisun/motty/commit/5bcf172))
- links under the pointer ([30cdfae](https://github.com/farvisun/motty/commit/30cdfae))
- vi mode and read-only mode ([5dcb1ff](https://github.com/farvisun/motty/commit/5dcb1ff))
- hint mode ([0eef2fe](https://github.com/farvisun/motty/commit/0eef2fe))
- session recovery ([9c7fe2c](https://github.com/farvisun/motty/commit/9c7fe2c))
- bash shell integration ([736cae2](https://github.com/farvisun/motty/commit/736cae2))
- reopen closed tabs ([608b3b4](https://github.com/farvisun/motty/commit/608b3b4))
- vi mode key hints ([9b5320b](https://github.com/farvisun/motty/commit/9b5320b))
- command palette and `motty action` ([2abe02b](https://github.com/farvisun/motty/commit/2abe02b))
- Open Quickly and frequent folders ([d6a681e](https://github.com/farvisun/motty/commit/d6a681e))
- custom keybindings ([30efba4](https://github.com/farvisun/motty/commit/30efba4))
- details panel ([3cdb10f](https://github.com/farvisun/motty/commit/3cdb10f))
- inline autocomplete from command history ([16d7340](https://github.com/farvisun/motty/commit/16d7340))
- app icon ([722c8c4](https://github.com/farvisun/motty/commit/722c8c4))
- first-run welcome guide ([54589a7](https://github.com/farvisun/motty/commit/54589a7))
- menu bar, right-click menu and settings window ([fd36df4](https://github.com/farvisun/motty/commit/fd36df4))
- edit shortcuts in settings, animate section changes ([39cbc27](https://github.com/farvisun/motty/commit/39cbc27))
- Otty's new-tab menu on the chevron ([58fcccd](https://github.com/farvisun/motty/commit/58fcccd))
- file editor tabs ([7b23628](https://github.com/farvisun/motty/commit/7b23628))
- agent state, tab badges and Inbox ([abb27ea](https://github.com/farvisun/motty/commit/abb27ea))
- Settings → Agents with hook installers for 12 agents ([25f14ed](https://github.com/farvisun/motty/commit/25f14ed))
- Otty's menus, tab menu, groups, global search and close confirmation ([1784bbd](https://github.com/farvisun/motty/commit/1784bbd))
- progress ring and Dock icon, paste protection, file drop, Secure Keyboard Entry and selection options ([68b2692](https://github.com/farvisun/motty/commit/68b2692))
- **agents:** composer, prompt queue, send to chat, history and skills ([1b9af4c](https://github.com/farvisun/motty/commit/1b9af4c))
- **titlebar:** Otty's title menu and a resizable sidebar ([ea14575](https://github.com/farvisun/motty/commit/ea14575))
- **menus:** Otty's menu bar and pop-up menus ([f0b33ec](https://github.com/farvisun/motty/commit/f0b33ec))
- **settings:** Otty's twelve sections and new config keys ([5c4fc67](https://github.com/farvisun/motty/commit/5c4fc67))
- **palette:** Otty's missing commands ([9634980](https://github.com/farvisun/motty/commit/9634980))
- **tabs:** sudo, SSH and caffeinate badges; Sudo, SSH and Pinned titlebar pills ([cad8142](https://github.com/farvisun/motty/commit/cad8142))
- **inbox:** Otty's Continue group ([404ef49](https://github.com/farvisun/motty/commit/404ef49))
- **tabs:** hover card ([bf18aec](https://github.com/farvisun/motty/commit/bf18aec))
- **open-quickly:** ⌘K actions for every row ([b372691](https://github.com/farvisun/motty/commit/b372691))
- **dock:** clicking the Dock icon goes to the next failing tab ([9cf9c1d](https://github.com/farvisun/motty/commit/9cf9c1d))
- **windows:** new windows cascade 28pt from the front one ([500b513](https://github.com/farvisun/motty/commit/500b513))
- **privileges:** Otty's notification, sound and shell-control privileges ([01d9257](https://github.com/farvisun/motty/commit/01d9257))
- **menus:** Otty's title, tab and path context menu items ([b8ce06d](https://github.com/farvisun/motty/commit/b8ce06d))
- **sidebar:** labelled dividers ([acba298](https://github.com/farvisun/motty/commit/acba298))
- **sidebar:** fold group sections ([9e547a6](https://github.com/farvisun/motty/commit/9e547a6))
- **sidebar:** auto-hide-tabs-panel (default, always, auto) ([4eddf87](https://github.com/farvisun/motty/commit/4eddf87))
- **closing:** idle coding agents in the close confirmation ([f53873f](https://github.com/farvisun/motty/commit/f53873f))
- **config:** include, Otty's lenient values, ~ and $VAR, local layering ([6f18d6c](https://github.com/farvisun/motty/commit/6f18d6c))
- **recovery:** "Closed at" / "Restored at" banners ([95730fd](https://github.com/farvisun/motty/commit/95730fd))
- **terminal:** pointer shapes from programs (OSC 22) ([b23a899](https://github.com/farvisun/motty/commit/b23a899))
- **open-quickly:** editor tabs and agent conversation search ([abff23b](https://github.com/farvisun/motty/commit/abff23b))
- **menus:** Git Merge/Rebase/worktrees/clients, Ports Other, Memory Report ([2aab549](https://github.com/farvisun/motty/commit/2aab549))
- **about-line:** the time a command ran ([876a3f2](https://github.com/farvisun/motty/commit/876a3f2))
- **search:** Find in All Tabs results tab and Search in Files ([eefb6d0](https://github.com/farvisun/motty/commit/eefb6d0))
- **recipes:** save, export, open and apply recipes; text snippets ([4067610](https://github.com/farvisun/motty/commit/4067610))
- **themes:** add 16 open-source built-in themes ([63e2c88](https://github.com/farvisun/motty/commit/63e2c88))
- **tabs:** add Phosphor duotone tab icons ([852fd40](https://github.com/farvisun/motty/commit/852fd40))
- tab rules, horizontal tab bar, theme picker, import and editor ([a7d2d10](https://github.com/farvisun/motty/commit/a7d2d10))
- **editor:** soft wrap, tab size, visible whitespace and Settings › Editor ([7dba9f7](https://github.com/farvisun/motty/commit/7dba9f7))
- **web:** native WKWebView element ([de26e77](https://github.com/farvisun/motty/commit/de26e77))
- **editor:** syntax highlighting with syntect ([0808a2a](https://github.com/farvisun/motty/commit/0808a2a))
- **preview:** Markdown, Mermaid and SVG previews ([c6454db](https://github.com/farvisun/motty/commit/c6454db))
- **editor:** Otty's file toolbar and syntax colors ([b2adb4e](https://github.com/farvisun/motty/commit/b2adb4e))
- **editor:** Preview for Markdown, SVG and HTML, reload, and crash-safe edits ([a005ed2](https://github.com/farvisun/motty/commit/a005ed2))
- **web:** web pane with Otty's toolbar ([a2ad242](https://github.com/farvisun/motty/commit/a2ad242))
- **folder:** Otty's folder pane as a standalone view ([3c1c8fa](https://github.com/farvisun/motty/commit/3c1c8fa))
- tell image, text and Quick Look files apart ([b56a781](https://github.com/farvisun/motty/commit/b56a781))
- image viewer pane ([545c80f](https://github.com/farvisun/motty/commit/545c80f))
- Quick Look pane for PDFs and binary files ([779ca89](https://github.com/farvisun/motty/commit/779ca89))
- **panes:** folder, image and Quick Look panes ([baafe21](https://github.com/farvisun/motty/commit/baafe21))
- **config:** details-view lines for the details panel's tabs ([2307435](https://github.com/farvisun/motty/commit/2307435))
- **details:** resizable panel, tab dropdown, custom views and richer tabs ([925acc1](https://github.com/farvisun/motty/commit/925acc1))
- **panes:** drag panes by their capsule, and tear tabs off ([9b8467f](https://github.com/farvisun/motty/commit/9b8467f))
- **view:** Picture in Picture ([dba577b](https://github.com/farvisun/motty/commit/dba577b))
- **editor:** Otty's diff view and language dropdown ([17db2fa](https://github.com/farvisun/motty/commit/17db2fa))
- **agents:** History view for agent session logs ([7ce7c5f](https://github.com/farvisun/motty/commit/7ce7c5f))
- **config:** agent-command lines for agent launch commands ([899fe58](https://github.com/farvisun/motty/commit/899fe58))
- **agents:** read Kimi Code session history ([61bdf53](https://github.com/farvisun/motty/commit/61bdf53))
- **agents:** start, resume and fork agents with their agent-command ([b578062](https://github.com/farvisun/motty/commit/b578062))
- **settings:** launch command and config home under Integrations ([92784eb](https://github.com/farvisun/motty/commit/92784eb))
- **agents:** open Kimi sessions as History, with their thinking ([ac7cde2](https://github.com/farvisun/motty/commit/ac7cde2))
- **composer:** rich paste ([c206bd8](https://github.com/farvisun/motty/commit/c206bd8))
- **composer:** Float Panel ([f213988](https://github.com/farvisun/motty/commit/f213988))
- **config:** agent-quota-off records agents with Show Plan Quota off ([c7c8b33](https://github.com/farvisun/motty/commit/c7c8b33))
- **agents:** read session usage from logs and plan quota from CLIs ([8aab5e0](https://github.com/farvisun/motty/commit/8aab5e0))
- **details:** usage readout and plan quota in the Agent group ([6b2e0e3](https://github.com/farvisun/motty/commit/6b2e0e3))
- **term:** term key and Otty's identification variables ([13379aa](https://github.com/farvisun/motty/commit/13379aa))
- **links:** motty:// URL scheme with line deeplinks and run links ([bfc6868](https://github.com/farvisun/motty/commit/bfc6868))
- **settings:** TERM row under Shell ([f517b56](https://github.com/farvisun/motty/commit/f517b56))
- **term:** a session's tty device, and all of its lines ([3fdd87c](https://github.com/farvisun/motty/commit/3fdd87c))
- **applescript:** Terminal.app-compatible scripting dictionary ([a0c8433](https://github.com/farvisun/motty/commit/a0c8433))
- **config:** key schema, config.local.toml and in-place write-back ([dedf582](https://github.com/farvisun/motty/commit/dedf582))
- **app:** config reload and transient values over IPC, env, user fonts ([8ae7001](https://github.com/farvisun/motty/commit/8ae7001))
- **cli:** config, theme and font commands ([a48469d](https://github.com/farvisun/motty/commit/a48469d))
- **cli:** import and export other terminals' configs ([007a434](https://github.com/farvisun/motty/commit/007a434))
- **config:** term in the schema and in Ghostty and kitty imports ([26f12fb](https://github.com/farvisun/motty/commit/26f12fb))
- **ipc:** add selectors, motty:// links and window/tab/pane methods ([57dd432](https://github.com/farvisun/motty/commit/57dd432))
- **app:** serve the window/tab/pane tree, jump, keybinds and recipes over IPC ([1224db9](https://github.com/farvisun/motty/commit/1224db9))
- **shell-integration:** let motty jump cd in the calling shell ([0932f4c](https://github.com/farvisun/motty/commit/0932f4c))
- **cli:** add Otty's command tree, keeping the flat verbs as hidden aliases ([64f3acf](https://github.com/farvisun/motty/commit/64f3acf))
- **vendor:** record modifyOtherKeys and KAM in the Term ([55add9e](https://github.com/farvisun/motty/commit/55add9e))
- **term:** encode keys for the kitty keyboard protocol and modifyOtherKeys ([426ef60](https://github.com/farvisun/motty/commit/426ef60))
- **term:** answer identification queries as Motty and reset key encodings after a command ([9d97430](https://github.com/farvisun/motty/commit/9d97430))
- **config:** add terminal identity and VT keys ([1e1bd87](https://github.com/farvisun/motty/commit/1e1bd87))
- **app:** send key releases, modifier keys and keypad keys to programs that ask ([0017ec8](https://github.com/farvisun/motty/commit/0017ec8))
- **term:** drop stale scrollback copies from repaints taller than the window ([ff277d5](https://github.com/farvisun/motty/commit/ff277d5))
- **config:** add Otty's scrolling, cursor animation and repaint dedup keys ([5367ad8](https://github.com/farvisun/motty/commit/5367ad8))
- **render:** draw the grid between rows and move the cursor ([9638de9](https://github.com/farvisun/motty/commit/9638de9))
- **terminal:** smooth scroll, output slide, scroll past the ends and a scrollbar ([627b931](https://github.com/farvisun/motty/commit/627b931))
- **config:** add Otty's text-rendering keys ([b9ee015](https://github.com/farvisun/motty/commit/b9ee015))
- **term:** track BDSM, keep SGR blink and widen ambiguous blocks ([78ba1fc](https://github.com/farvisun/motty/commit/78ba1fc))
- **render:** add bidi reordering, contrast, wrap-pad and arrow-join helpers ([8ede973](https://github.com/farvisun/motty/commit/8ede973))
- **render:** draw bidi, faces, blink, contrast and cell height ([e51120f](https://github.com/farvisun/motty/commit/e51120f))
- **app:** blink text and map clicks through the visual line order ([ec82f27](https://github.com/farvisun/motty/commit/ec82f27))
- **settings:** add text rendering rows ([0295b1a](https://github.com/farvisun/motty/commit/0295b1a))
- **term:** kitty graphics, iTerm2 and sixel inline images ([bfef0a8](https://github.com/farvisun/motty/commit/bfef0a8))
- **render:** paint inline images below and above text ([e3dd5cc](https://github.com/farvisun/motty/commit/e3dd5cc))
- **app:** hand the session's images to the terminal element ([d8d89d1](https://github.com/farvisun/motty/commit/d8d89d1))

### Bug fixes

- **theme:** parse config line by line and derive native titlebar colors ([dc90e83](https://github.com/farvisun/motty/commit/dc90e83))
- **render:** match otty cell metrics and baseline ([60471b7](https://github.com/farvisun/motty/commit/60471b7))
- **theme:** default sidebar tab titles to 12pt like otty ([eb65c6e](https://github.com/farvisun/motty/commit/eb65c6e))
- **theme:** let a theme's terminal colors win over config colors ([da1bba8](https://github.com/farvisun/motty/commit/da1bba8))
- **theme:** sections with a material default to a transparent fill ([6560990](https://github.com/farvisun/motty/commit/6560990))
- **app:** draw tab cards like otty across themes ([5fcdbd0](https://github.com/farvisun/motty/commit/5fcdbd0))
- **xtask:** park the pointer outside the window before captures ([0195f18](https://github.com/farvisun/motty/commit/0195f18))
- **theme:** derive sidebar and active tab like Otty ([9d06369](https://github.com/farvisun/motty/commit/9d06369))
- **xtask:** capture only the window and its backdrop ([170fcd6](https://github.com/farvisun/motty/commit/170fcd6))
- **xtask:** track the Otty fixture window by id ([d7babe2](https://github.com/farvisun/motty/commit/d7babe2))
- **app:** native titlebar height beside the sidebar ([d2b9a70](https://github.com/farvisun/motty/commit/d2b9a70))
- **app:** sidebar padding insets; no shadows through translucent cards ([001f038](https://github.com/farvisun/motty/commit/001f038))
- **app:** CSS shadow blur is twice GPUI's sigma ([5dc4d83](https://github.com/farvisun/motty/commit/5dc4d83))
- **app:** start each PTY at its pane's size ([23638df](https://github.com/farvisun/motty/commit/23638df))
- truncate long window titles in the titlebar ([1327854](https://github.com/farvisun/motty/commit/1327854))
- keep window shortcuts out of Open Quickly ([dc5afad](https://github.com/farvisun/motty/commit/dc5afad))
- let test windows draw without taking the keyboard ([c5c02ad](https://github.com/farvisun/motty/commit/c5c02ad))
- **palette:** work over editor tabs, one header per group; find bar has Otty's two toggles ([9e03e4b](https://github.com/farvisun/motty/commit/9e03e4b))
- **cli:** open works while a file pane has focus ([5763e45](https://github.com/farvisun/motty/commit/5763e45))
- **quick-look:** hide under GPUI overlays like web views ([78244fb](https://github.com/farvisun/motty/commit/78244fb))
- **details:** follow folder, web, image and Quick Look panes ([8185269](https://github.com/farvisun/motty/commit/8185269))
- **details:** say when a folder isn't a git repository ([bb77f89](https://github.com/farvisun/motty/commit/bb77f89))
- **agents:** build the quota poll and Kimi log lookup on main ([1a06514](https://github.com/farvisun/motty/commit/1a06514))
- **applescript:** a window's name is its titlebar title, renamed or not ([e10461b](https://github.com/farvisun/motty/commit/e10461b))
- **render:** slide a batch of output in whole ([81f5211](https://github.com/farvisun/motty/commit/81f5211))

### Refactoring

- share the picker card between palettes ([0ed1130](https://github.com/farvisun/motty/commit/0ed1130))
- **panes:** a pane can hold a file as well as a terminal ([cd1c0e5](https://github.com/farvisun/motty/commit/cd1c0e5))
- **web-view:** share the native overlay with other panes ([88eb924](https://github.com/farvisun/motty/commit/88eb924))
- **links:** resolve motty:// targets with the CLI's selectors ([d71783c](https://github.com/farvisun/motty/commit/d71783c))

### Documentation

- teach the new motty commands in skills and the README ([a254cd9](https://github.com/farvisun/motty/commit/a254cd9))
- list the scrolling, cursor animation and repaint dedup keys ([065a798](https://github.com/farvisun/motty/commit/065a798))

### Build

- vendor alacritty_terminal with a pushed-lines counter ([6d68c3b](https://github.com/farvisun/motty/commit/6d68c3b))
- **xtask:** bundle the AppleScript dictionary ([0919404](https://github.com/farvisun/motty/commit/0919404))

[Unreleased]: https://github.com/farvisun/motty/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/farvisun/motty/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/farvisun/motty/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/farvisun/motty/releases/tag/v0.1.0
