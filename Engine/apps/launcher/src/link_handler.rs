#[cfg(not(windows))]
use std::path::{Path, PathBuf};

#[cfg(not(windows))]
pub const DESKTOP_FILE: &str = "project-avalanche-link.desktop";
#[cfg(not(windows))]
const MIME: &str = "x-scheme-handler/avalanche";
#[cfg(not(windows))]
const SECTION: &str = "[Default Applications]";
// PORT(choice): 1 MiB (a desktop's list is a few KiB; the specification states no limit), the port's reader; the
// original has no launcher.
#[cfg(not(windows))]
const MAX_LIST_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ours,
    None,
    Other(String),
    Unavailable(String),
}

#[cfg(not(windows))]
pub fn exec_quoted(path: &Path) -> String {
    let mut out = String::from("\"");
    for c in path.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(not(windows))]
pub fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Project Avalanche (server links)\nExec={} --link %u\n\
         MimeType={MIME};\nNoDisplay=true\nTerminal=false\n",
        exec_quoted(exe)
    )
}

#[cfg(not(windows))]
pub fn edit_mimeapps(text: &str, handler: Option<&str>) -> Option<String> {
    let cr = if text.contains("\r\n") { "\r" } else { "" };
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let ended = text.ends_with('\n');
    if ended || text.is_empty() {
        lines.pop();
    }
    let mut section: Option<usize> = None;
    let mut found: Option<usize> = None;
    let mut current = "";
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if t.starts_with('[') {
            current = if t == SECTION { SECTION } else { "" };
            if current == SECTION && section.is_none() {
                section = Some(i);
            }
            continue;
        }
        if current == SECTION && t.split_once('=').is_some_and(|(k, _)| k.trim() == MIME) {
            found = Some(i);
        }
    }
    let ours = format!("{MIME}={DESKTOP_FILE};");
    match (handler, found) {
        (Some(_), Some(i)) if lines[i].trim() == ours => return None,
        (Some(_), Some(i)) => lines[i] = format!("{ours}{cr}"),
        (Some(_), None) => match section {
            Some(s) => lines.insert(s + 1, format!("{ours}{cr}")),
            None => {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(cr.to_string());
                }
                lines.push(format!("{SECTION}{cr}"));
                lines.push(format!("{ours}{cr}"));
            }
        },
        (None, Some(i)) => {
            let value = lines[i].split_once('=').map_or("", |(_, v)| v).trim().trim_end_matches(';');
            if value.split(';').map(str::trim).any(|d| d == DESKTOP_FILE) {
                lines.remove(i);
            } else {
                return None;
            }
        }
        (None, None) => return None,
    }
    let mut out = lines.join("\n");
    if ended || text.is_empty() {
        out.push('\n');
    }
    Some(out)
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    fn home_dir(var: &str, fallback: &str) -> Option<PathBuf> {
        match std::env::var_os(var).filter(|v| !v.is_empty()) {
            Some(v) => Some(PathBuf::from(v)),
            None => std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(fallback)),
        }
    }

    fn paths() -> Option<(PathBuf, PathBuf)> {
        let data = home_dir("XDG_DATA_HOME", ".local/share")?;
        let config = home_dir("XDG_CONFIG_HOME", ".config")?;
        Some((data.join("applications").join(DESKTOP_FILE), config.join("mimeapps.list")))
    }

    fn read_capped(path: &Path) -> Result<Option<String>, String> {
        use std::io::Read;
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut bytes = Vec::new();
        file.take(MAX_LIST_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if bytes.len() as u64 > MAX_LIST_BYTES {
            return Err(format!("{}: larger than {MAX_LIST_BYTES} bytes", path.display()));
        }
        String::from_utf8(bytes).map(Some).map_err(|_| format!("{}: not UTF-8 text", path.display()))
    }

    fn write_whole(path: &Path, text: &str) -> Result<(), String> {
        let parent = path.parent().ok_or("no folder")?;
        ssx_files::create_dir_all(parent).map_err(|e| e.to_string())?;
        ssx_files::replace(path, text.as_bytes(), &ssx_files::Options::new()).map_err(|e| e.to_string())
    }

    pub fn status(exe: &Path) -> Status {
        let Some((desktop, mimeapps)) = paths() else { return Status::Unavailable("no home folder".into()) };
        status_at(&desktop, &mimeapps, exe)
    }

    pub fn status_at(desktop: &Path, mimeapps: &Path, exe: &Path) -> Status {
        let (file, list) = match (read_capped(desktop), read_capped(mimeapps)) {
            (Ok(f), Ok(l)) => (f.unwrap_or_default(), l.unwrap_or_default()),
            (Err(e), _) | (_, Err(e)) => return Status::Unavailable(e),
        };
        let ours = file == desktop_entry(exe);
        match edit_mimeapps(&list, Some(DESKTOP_FILE)) {
            None if ours => Status::Ours,
            None => Status::Other(format!("{DESKTOP_FILE} names another program")),
            Some(_) => {
                let other = list.lines().find(|l| l.trim_start().starts_with(MIME)).map(str::to_string);
                other.map_or(Status::None, Status::Other)
            }
        }
    }

    pub fn register(exe: &Path) -> Result<String, String> {
        let (desktop, mimeapps) = paths().ok_or("no home folder")?;
        register_at(&desktop, &mimeapps, exe)
    }

    pub fn register_at(desktop: &Path, mimeapps: &Path, exe: &Path) -> Result<String, String> {
        let list = read_capped(mimeapps)?.unwrap_or_default();
        write_whole(desktop, &desktop_entry(exe))?;
        if let Some(text) = edit_mimeapps(&list, Some(DESKTOP_FILE)) {
            write_whole(mimeapps, &text)?;
        }
        Ok(format!("wrote {} and the line {MIME}={DESKTOP_FILE}; in {}", desktop.display(), mimeapps.display()))
    }

    pub fn unregister(_exe: &Path) -> Result<String, String> {
        let (desktop, mimeapps) = paths().ok_or("no home folder")?;
        unregister_at(&desktop, &mimeapps)
    }

    pub fn unregister_at(desktop: &Path, mimeapps: &Path) -> Result<String, String> {
        let mut done = Vec::new();
        let list = read_capped(mimeapps)?.unwrap_or_default();
        if let Some(text) = edit_mimeapps(&list, None) {
            write_whole(mimeapps, &text)?;
            done.push(format!("removed the line {MIME}={DESKTOP_FILE}; from {}", mimeapps.display()));
        }
        match std::fs::remove_file(desktop) {
            Ok(()) => done.push(format!("deleted {}", desktop.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", desktop.display())),
        }
        Ok(if done.is_empty() { "nothing to remove".into() } else { done.join("; ") })
    }
}

#[cfg(windows)]
#[path = "link_handler_windows.rs"]
mod platform;

pub fn status() -> Status {
    match std::env::current_exe() {
        Ok(exe) => platform::status(&exe),
        Err(e) => Status::Unavailable(format!("the launcher's own path: {e}")),
    }
}

pub fn register() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("the launcher's own path: {e}"))?;
    platform::register(&exe)
}

pub fn unregister() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("the launcher's own path: {e}"))?;
    platform::unregister(&exe)
}

pub const MAX_LINK: usize = "avalanche://connect/".len() + 262;

pub fn link_start_asked(words: &[String]) -> bool {
    words.iter().any(|w| w == "--link")
}

pub fn link_start(words: &[String]) -> Result<String, String> {
    match words {
        [flag, link] if flag == "--link" => strict_link(link).map(|()| link.clone()),
        _ => Err(format!(
            "a start for a link takes --link and the link and nothing else; this one was given {} words: {}",
            words.len(),
            shown_words(words)
        )),
    }
}

fn shown_words(words: &[String]) -> String {
    let all = words.iter().map(|w| ssx_core::text::quoted_max(w, 400).to_string()).collect::<Vec<_>>().join(" ");
    ssx_core::text::shown_max(&all, 400).to_string()
}

pub fn strict_link(link: &str) -> Result<(), String> {
    if link.len() > MAX_LINK {
        return Err(format!("the link has more than {MAX_LINK} characters"));
    }
    if !link.is_ascii() {
        return Err("the link has a character outside ASCII".into());
    }
    let address = match link.get(..12) {
        Some(scheme) if scheme.eq_ignore_ascii_case("avalanche://") => link[12..].strip_prefix("connect/"),
        _ => None,
    }
    .ok_or("the link is not avalanche://connect/host:port")?;
    if let Some(c) = address.chars().find(|&c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']')))
    {
        return Err(format!("the link has {c:?} in its address (only host:port may follow connect/)"));
    }
    let port = match address.strip_prefix('[') {
        Some(rest) => {
            let (ip, after) = rest.split_once(']').ok_or("the link's IPv6 address has no closing ]")?;
            if ip.is_empty() || ip.contains('[') {
                return Err("the link's IPv6 address is not one".into());
            }
            after.strip_prefix(':').ok_or("the link's address has no port")?
        }
        None => {
            if address.contains(['[', ']']) {
                return Err("the link's address has a bracket outside an IPv6 address".into());
            }
            let (host, port) = address.split_once(':').ok_or("the link's address has no port")?;
            if host.is_empty() {
                return Err("the link's address has no host".into());
            }
            port
        }
    };
    let ok = !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit()) && !port.starts_with('0');
    match ok.then(|| port.parse::<u16>().ok()).flatten() {
        Some(_) => Ok(()),
        None => Err(format!("the link's port {port:?} is not 1 to 65535")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn the_exec_line_quotes_as_the_spec_asks() {
        use std::path::Path;
        assert_eq!(
            exec_quoted(Path::new("/opt/Project Avalanche/project-avalanche")),
            "\"/opt/Project Avalanche/project-avalanche\""
        );
        assert_eq!(exec_quoted(Path::new("/a\"b$c`d\\e%f")), "\"/a\\\"b\\$c\\`d\\\\e%%f\"");
        assert!(desktop_entry(Path::new("/x")).contains("Exec=\"/x\" --link %u\n"));
    }

    #[cfg(not(windows))]
    #[test]
    fn mimeapps_keeps_every_other_line() {
        let list = "[Added Associations]\ntext/plain=gedit.desktop;\n\n[Default Applications]\nx-scheme-handler/http=firefox.desktop\n";
        let on = edit_mimeapps(list, Some(DESKTOP_FILE)).expect("added");
        assert_eq!(
            on,
            "[Added Associations]\ntext/plain=gedit.desktop;\n\n[Default Applications]\nx-scheme-handler/avalanche=project-avalanche-link.desktop;\nx-scheme-handler/http=firefox.desktop\n"
        );
        assert_eq!(edit_mimeapps(&on, Some(DESKTOP_FILE)), None, "once");
        assert_eq!(edit_mimeapps(&on, None).as_deref(), Some(list));
        let empty = edit_mimeapps("", Some(DESKTOP_FILE)).expect("a section made");
        assert_eq!(empty, "[Default Applications]\nx-scheme-handler/avalanche=project-avalanche-link.desktop;\n");
        let other = "[Default Applications]\nx-scheme-handler/avalanche=someone-else.desktop;\n";
        assert_eq!(edit_mimeapps(other, None), None, "another program's handler is left");
        assert!(edit_mimeapps(other, Some(DESKTOP_FILE)).is_some_and(|t| t.contains(DESKTOP_FILE)), "taken when asked");
    }

    #[cfg(not(windows))]
    #[test]
    fn registration_writes_and_removes_exactly_its_own_lines() {
        use super::platform::{register_at, status_at, unregister_at};
        let dir = std::env::temp_dir().join(format!("ava-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (desktop, mimeapps) = (dir.join("data/applications").join(DESKTOP_FILE), dir.join("config/mimeapps.list"));
        std::fs::create_dir_all(mimeapps.parent().unwrap()).unwrap();
        let before = "[Default Applications]\nx-scheme-handler/http=firefox.desktop\n";
        std::fs::write(&mimeapps, before).unwrap();
        let exe = Path::new("/opt/Project Avalanche/project-avalanche");
        assert_eq!(status_at(&desktop, &mimeapps, exe), Status::None);
        register_at(&desktop, &mimeapps, exe).unwrap();
        assert_eq!(status_at(&desktop, &mimeapps, exe), Status::Ours);
        assert_eq!(std::fs::read_to_string(&desktop).unwrap(), desktop_entry(exe));
        unregister_at(&desktop, &mimeapps).unwrap();
        assert!(!desktop.exists());
        assert_eq!(std::fs::read_to_string(&mimeapps).unwrap(), before);
        assert_eq!(status_at(&desktop, &mimeapps, exe), Status::None);
        register_at(&desktop, &mimeapps, exe).unwrap();
        let on = std::fs::read_to_string(&mimeapps).unwrap();
        std::fs::write(&mimeapps, vec![b'#'; (MAX_LIST_BYTES + 1) as usize]).unwrap();
        assert!(matches!(status_at(&desktop, &mimeapps, exe), Status::Unavailable(e) if e.contains("larger than")));
        assert!(unregister_at(&desktop, &mimeapps).is_err_and(|e| e.contains("larger than")));
        assert!(desktop.exists(), "the desktop file stays while its line cannot be removed");
        std::fs::write(&mimeapps, [0xff, 0xfe, b'\n']).unwrap();
        assert!(register_at(&desktop, &mimeapps, exe).is_err_and(|e| e.contains("not UTF-8")));
        std::fs::write(&mimeapps, &on).unwrap();
        unregister_at(&desktop, &mimeapps).unwrap();
        assert_eq!(std::fs::read_to_string(&mimeapps).unwrap(), before);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn mimeapps_keeps_its_line_endings() {
        let crlf = "[Default Applications]\r\nx-scheme-handler/http=firefox.desktop\r\n";
        let on = edit_mimeapps(crlf, Some(DESKTOP_FILE)).expect("added");
        assert_eq!(
            on,
            "[Default Applications]\r\nx-scheme-handler/avalanche=project-avalanche-link.desktop;\r\n\
             x-scheme-handler/http=firefox.desktop\r\n"
        );
        assert_eq!(edit_mimeapps(&on, None).as_deref(), Some(crlf));
        let open_end = "[Added Associations]\ntext/plain=gedit.desktop;";
        let on = edit_mimeapps(open_end, Some(DESKTOP_FILE)).expect("added");
        assert!(on.ends_with("project-avalanche-link.desktop;") && on.starts_with(open_end), "{on:?}");
    }

    fn windows_argv(line: &str) -> Vec<String> {
        let mut chars = line.chars().peekable();
        let mut out = Vec::new();
        let mut program = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                program.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' || c == '\t' {
                    break;
                }
                program.push(c);
                chars.next();
            }
        }
        out.push(program);
        let (mut word, mut quoted, mut any) = (String::new(), false, false);
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    let mut n = 1;
                    while chars.peek() == Some(&'\\') {
                        chars.next();
                        n += 1;
                    }
                    if chars.peek() == Some(&'"') {
                        word.extend(std::iter::repeat_n('\\', n / 2));
                        if n % 2 == 1 {
                            chars.next();
                            word.push('"');
                        }
                    } else {
                        word.extend(std::iter::repeat_n('\\', n));
                    }
                    any = true;
                }
                '"' if quoted && chars.peek() == Some(&'"') => {
                    chars.next();
                    word.push('"');
                }
                '"' => {
                    quoted = !quoted;
                    any = true;
                }
                ' ' | '\t' if !quoted => {
                    if any {
                        out.push(std::mem::take(&mut word));
                        any = false;
                    }
                }
                c => {
                    word.push(c);
                    any = true;
                }
            }
        }
        if any {
            out.push(word);
        }
        out
    }

    fn windows_words(link: &str) -> Vec<String> {
        let line = format!("\"C:\\Program Files\\Project Avalanche\\project-avalanche.exe\" --link \"{link}\"");
        windows_argv(&line).split_off(1)
    }

    #[test]
    fn the_splitter_follows_the_documented_rules() {
        assert_eq!(windows_argv(r#""C:\a b\x.exe" a "b c" d"#), ["C:\\a b\\x.exe", "a", "b c", "d"]);
        assert_eq!(windows_argv(r#"x.exe a\\"b c" d"#), ["x.exe", "a\\b c", "d"]);
        assert_eq!(windows_argv(r#"x.exe a\\\"b"#), ["x.exe", "a\\\"b"]);
        assert_eq!(windows_argv(r#"x.exe "a""b" c\d"#), ["x.exe", "a\"b", "c\\d"]);
    }

    #[test]
    fn a_link_cannot_carry_other_options() {
        let ok = "avalanche://connect/127.0.0.1:27450";
        assert_eq!(link_start(&windows_words(ok)), Ok(ok.to_string()));
        assert_eq!(link_start(&windows_words("AVALANCHE://connect/[::1]:27450")).map(|_| ()), Ok(()));
        assert!(link_start(&windows_words("avalanche://connect/alps.example.org:27500")).is_ok());
        for link in [
            r#"avalanche://connect/x:1" --install "\\host\share\p.avapkg"#,
            r#"avalanche://connect/x:1" --dump-art "C:\Users\Public"#,
            r#"avalanche://connect/x:1" --screenshot "C:\x.png"#,
            r#"avalanche://connect/x:1" "--print"#,
            r#"avalanche://connect/x:1" --page mods --install "\\h\s\p"#,
            r#"x" --install "\\host\share\p.avapkg" --link "avalanche://connect/x:1"#,
        ] {
            let words = windows_words(link);
            assert!(words.len() > 2, "{link:?} split into {words:?}");
            assert!(link_start_asked(&words), "{words:?}");
            let e = link_start(&words).unwrap_err();
            assert!(e.contains("nothing else"), "{link:?}: {e}");
        }
        for link in [r#"avalanche://connect/x:1\" --print"#, r#"avalanche://connect/x:1"" --print"#] {
            let words = windows_words(link);
            assert!(link_start(&words).is_err(), "{link:?} → {words:?}");
        }
        for link in [
            r#"avalanche://connect/x:1" --install "/tmp/p.avapkg"#,
            "avalanche://connect/x:1 --install /tmp/p.avapkg",
            "avalanche://connect/x:1\n--install",
        ] {
            assert!(link_start(&["--link".to_string(), link.to_string()]).is_err(), "{link:?}");
            let split: Vec<String> = ["--link"].into_iter().chain(link.split(' ')).map(str::to_string).collect();
            assert!(link_start(&split).is_err(), "{split:?}");
        }
        for words in [
            vec!["--install", "/tmp/p.avapkg", "--link", ok],
            vec!["--link", ok, "--screenshot", "x.png"],
            vec!["--link", ok, "--link", ok],
            vec!["--link"],
        ] {
            let words: Vec<String> = words.into_iter().map(str::to_string).collect();
            assert!(link_start_asked(&words) && link_start(&words).is_err(), "{words:?}");
        }
        assert!(!link_start_asked(&["--play".to_string()]));
    }

    #[test]
    fn links_are_checked_strictly_before_they_reach_the_game() {
        for good in [
            "avalanche://connect/127.0.0.1:27450",
            "Avalanche://connect/[2001:db8::1]:1",
            "avalanche://connect/a-b.example:65535",
        ] {
            assert_eq!(strict_link(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "--password-file /etc/passwd",
            "http://connect/x:1",
            "avalanche://join/x:1",
            "avalanche://Connect/x:1",
            "avalanche://connect/",
            "avalanche://connect/x",
            "avalanche://connect/x:",
            "avalanche://connect/x:0",
            "avalanche://connect/x:01",
            "avalanche://connect/x:65536",
            "avalanche://connect/x:1/",
            "avalanche://connect/x:1?p=1",
            "avalanche://connect/x:1#a",
            "avalanche://connect/u@x:1",
            "avalanche://connect/x%3A1",
            "avalanche://connect/x:1\"",
            "avalanche://connect/x:1 ",
            "avalanche://connect/x:1\\",
            "avalanche://connect/x:1\t",
            "avalanche://connect/x:1\n--seconds",
            "avalanche://connect/\u{e9}.example:1",
            "avalanche://connect/[::1:1",
            "avalanche://connect/[]:1",
            "avalanche://connect/x]:1",
            "avalanche://connect/:1",
            "avalanche://connect/x:1:2",
        ] {
            assert!(strict_link(bad).is_err(), "{bad:?}");
        }
        assert!(strict_link(&format!("avalanche://connect/{}:1", "a".repeat(MAX_LINK))).is_err());
    }
}
