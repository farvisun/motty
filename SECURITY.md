# Security

Motty runs your shell and the programs in it, so a few things are guarded on purpose:

- The `motty` command talks to the app over a socket in your own Application Support (or per-user temporary) folder, which other users can't open. Typing into panes over that socket is off unless you turn on `ipc-allow-send-keys`.
- `motty://` run links never run anything by themselves: Motty asks first, then types the command at the prompt without pressing Return.
- Updates are downloaded from GitHub releases and installed only when their Ed25519 signature matches Motty's release key, besides the SHA-256 the release lists.

If you find a way around any of that, or anything else that lets a program, link or download act without your say, please report it privately through [GitHub security advisories](https://github.com/farvisun/motty/security/advisories/new) rather than in a public issue.
