//! What a pane's processes say about it beyond the program name: a sudo
//! session, an SSH connection, caffeinate keeping the Mac awake. Drives the
//! shield, devices and coffee-cup tab badges and the titlebar pills.

/// Read from the pane's process tree while a command runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemState {
    /// `sudo`, `su` or `doas` is running.
    pub sudo: bool,
    /// Connected to another host: its destination as typed (`user@host`),
    /// empty when it couldn't be read.
    pub ssh: Option<String>,
    /// `caffeinate` is keeping the system awake.
    pub caffeinate: bool,
}

impl SystemState {
    /// From the processes under the shell (the shell itself excluded) and a
    /// way to read a process's arguments.
    pub fn from_processes<'a>(
        procs: impl IntoIterator<Item = (i32, &'a str)>,
        args: impl Fn(i32) -> Option<Vec<String>>,
    ) -> Self {
        let mut state = Self::default();
        for (pid, name) in procs {
            match name {
                "sudo" | "su" | "doas" => state.sudo = true,
                "caffeinate" => state.caffeinate = true,
                "ssh" | "mosh-client" | "et" if state.ssh.is_none() => {
                    let dest = (name == "ssh")
                        .then(|| args(pid))
                        .flatten()
                        .and_then(|a| ssh_destination(&a));
                    state.ssh = Some(dest.unwrap_or_default());
                }
                _ => {}
            }
        }
        state
    }
}

/// The destination of an `ssh` command line (`argv[0]` included): the first
/// argument that isn't an option or an option's value.
pub fn ssh_destination(args: &[String]) -> Option<String> {
    // Options that take a value (ssh(1) synopsis).
    const WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";
    let mut it = args.iter().skip(1);
    while let Some(arg) = it.next() {
        let Some(flags) = arg.strip_prefix('-') else {
            return Some(arg.clone());
        };
        if flags == "-" {
            return it.next().cloned();
        }
        // `-p22` carries its value; `-p 22` takes the next argument.
        for (i, c) in flags.char_indices() {
            if WITH_VALUE.contains(c) {
                if i + c.len_utf8() == flags.len() {
                    it.next();
                }
                break;
            }
        }
    }
    None
}

/// A process's arguments, `argv[0]` first (`KERN_PROCARGS2`).
#[cfg(target_os = "macos")]
pub fn process_args(pid: i32) -> Option<Vec<String>> {
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `argmax` is a writable c_int of `size` bytes.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut argmax).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: the buffer is valid for `size` bytes.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 {
        return None;
    }
    buf.truncate(size);
    parse_procargs(&buf)
}

#[cfg(not(target_os = "macos"))]
pub fn process_args(_pid: i32) -> Option<Vec<String>> {
    None
}

/// `argc`, the executable path, NUL padding, then `argc` NUL-terminated
/// arguments.
fn parse_procargs(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let rest = &buf[4..];
    let mut i = rest.iter().position(|&b| b == 0)?;
    while rest.get(i) == Some(&0) {
        i += 1;
    }
    Some(
        rest[i..]
            .split(|&b| b == 0)
            .take(argc)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(line: &str) -> Option<String> {
        let args: Vec<String> = line.split(' ').map(str::to_owned).collect();
        ssh_destination(&args)
    }

    #[test]
    fn destinations() {
        assert_eq!(dest("ssh box").as_deref(), Some("box"));
        assert_eq!(dest("ssh -p 2222 me@box ls").as_deref(), Some("me@box"));
        assert_eq!(dest("ssh -p2222 -A box").as_deref(), Some("box"));
        assert_eq!(dest("ssh -i key -tt box").as_deref(), Some("box"));
        assert_eq!(dest("ssh -- box").as_deref(), Some("box"));
        assert_eq!(dest("ssh -v"), None);
    }

    #[test]
    fn states() {
        let s = SystemState::from_processes([(2, "sudo"), (3, "ssh"), (4, "caffeinate")], |_| {
            Some(vec!["ssh".into(), "-q".into(), "host".into()])
        });
        assert!(s.sudo && s.caffeinate);
        assert_eq!(s.ssh.as_deref(), Some("host"));
        assert_eq!(
            SystemState::from_processes([(2, "vim")], |_| None),
            SystemState::default()
        );
    }

    #[test]
    fn procargs() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend(b"/usr/bin/ssh\0\0\0ssh\0box\0SHELL=zsh\0");
        assert_eq!(parse_procargs(&buf).unwrap(), vec!["ssh", "box"]);
    }
}
