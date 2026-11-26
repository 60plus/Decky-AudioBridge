//! Windows integration: start with Windows, firewall rule, opening folders.
//! On other systems these are no-ops so the core can be tested anywhere.

const RULE: &str = "Audio Bridge Receiver";

/// The netsh command line that replaces this program's own firewall rules
/// with one rule: incoming UDP on `port`, from the local subnet only, on
/// private and domain networks, plus public ones when `include_public`.
#[cfg_attr(not(windows), allow(dead_code))]
fn firewall_script(path: &str, port: u16, include_public: bool) -> String {
    let profile = if include_public {
        "any"
    } else {
        "private,domain"
    };
    format!(
        "/c netsh advfirewall firewall delete rule name=all program=\"{path}\" & \
         netsh advfirewall firewall add rule name=\"{RULE}\" dir=in action=allow protocol=UDP \
         localport={port} remoteip=localsubnet profile={profile} program=\"{path}\""
    )
}

#[cfg(windows)]
mod imp {
    use super::firewall_script;
    use std::os::windows::ffi::OsStrExt;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "AudioBridgeReceiver";

    fn exe() -> Option<String> {
        std::env::current_exe()
            .ok()
            .map(|p| p.display().to_string())
    }

    fn command_line() -> Option<String> {
        exe().map(|path| format!("\"{path}\" --minimized"))
    }

    pub fn autostart_enabled() -> bool {
        let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ)
        else {
            return false;
        };
        let value: Result<String, _> = key.get_value(VALUE);
        match (value, command_line()) {
            (Ok(v), Some(cmd)) => v.eq_ignore_ascii_case(&cmd),
            _ => false,
        }
    }

    pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE)?;
        if enabled {
            let cmd = command_line()
                .ok_or_else(|| std::io::Error::other("cannot find this program's path"))?;
            key.set_value(VALUE, &cmd)
        } else {
            match key.delete_value(VALUE) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }

    fn shell_execute(verb: &str, file: &str, params: &str, show: i32) -> bool {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let (verb, file, params) = (wide(verb), wide(file), wide(params));
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                params.as_ptr(),
                std::ptr::null(),
                show,
            )
        };
        result as isize > 32
    }

    /// Ask Windows (with a UAC prompt) to replace the rules for this program
    /// only, including a "block" rule left by a cancelled firewall question,
    /// with one rule: incoming UDP on the receiver port, from the local subnet.
    /// By default it covers networks Windows calls private or domain;
    /// `include_public` extends it to networks marked public.
    pub fn allow_through_firewall(port: u16, include_public: bool) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
        let Some(path) = exe() else { return false };
        let script = firewall_script(&path, port, include_public);
        shell_execute("runas", "cmd.exe", &script, SW_HIDE)
    }

    pub fn open_folder(path: &std::path::Path) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        shell_execute("open", &path.display().to_string(), "", SW_SHOWNORMAL)
    }

    pub fn error_box(message: &str) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let (text, title) = (wide(message), wide("Audio Bridge Receiver"));
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn autostart_enabled() -> bool {
        false
    }
    pub fn set_autostart(_enabled: bool) -> std::io::Result<()> {
        Err(std::io::Error::other("only available on Windows"))
    }
    pub fn allow_through_firewall(_port: u16, _include_public: bool) -> bool {
        false
    }
    pub fn open_folder(_path: &std::path::Path) -> bool {
        false
    }
    pub fn error_box(message: &str) {
        eprintln!("{message}");
    }
}

pub use imp::*;

pub const AUTOSTART_SUPPORTED: bool = cfg!(windows);

#[cfg(test)]
mod tests {
    use super::firewall_script;

    #[test]
    fn firewall_rule_is_narrow() {
        let private = firewall_script(r"C:\Apps\AudioBridgeReceiver.exe", 6980, false);
        assert!(private.contains("profile=private,domain "));
        assert!(private.contains("remoteip=localsubnet"));
        assert!(private.contains("protocol=UDP localport=6980"));
        assert!(
            private.contains(r#"delete rule name=all program="C:\Apps\AudioBridgeReceiver.exe""#)
        );
        assert!(!private.contains("profile=any"));
        let public = firewall_script(r"C:\Apps\AudioBridgeReceiver.exe", 7000, true);
        assert!(public.contains("profile=any "));
        assert!(public.contains("remoteip=localsubnet"));
        assert!(public.contains("localport=7000"));
    }
}
