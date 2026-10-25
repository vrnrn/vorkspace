//! Relaunch only after this process exits, including its asynchronous quit hooks.
use std::{
    ffi::OsString,
    io,
    path::Path,
    process::{Command, Stdio},
};

pub(crate) fn relaunch_self() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();

    #[cfg(target_os = "macos")]
    {
        let installed = Path::new("/Applications/vorkspace.app");
        let bundle = if installed.exists() {
            Some(installed)
        } else {
            exe.parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        };
        if let Some(bundle) = bundle {
            let mut open_args = vec![OsString::from("-n"), bundle.as_os_str().to_owned()];
            if !args.is_empty() {
                open_args.push(OsString::from("--args"));
                open_args.extend(args);
            }
            return wait_and_relaunch(std::process::id(), Path::new("/usr/bin/open"), &open_args);
        }
    }

    wait_and_relaunch(std::process::id(), &exe, &args)
}

pub(crate) fn wait_and_relaunch(pid: u32, exe: &Path, args: &[OsString]) -> io::Result<()> {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        // Paths and app arguments are positional parameters, never shell source.
        command
            .args([
                "-c",
                "while kill -0 \"$1\" 2>/dev/null; do sleep 0.1; done; shift; exec \"$@\"",
                "vorkspace-restart",
            ])
            .arg(pid.to_string())
            .arg(exe)
            .args(args);
        command
    };

    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let arguments = args
            .iter()
            .map(|arg| windows_argument(&arg.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        let script = format!(
            "$ErrorActionPreference = 'Stop'; \
             $old = Get-Process -Id {pid} -ErrorAction SilentlyContinue; \
             if ($null -ne $old) {{ $old.WaitForExit() }}; \
             $start = New-Object System.Diagnostics.ProcessStartInfo; \
             $start.UseShellExecute = $false; \
             $start.FileName = '{}'; $start.Arguments = '{}'; \
             [System.Diagnostics.Process]::Start($start) | Out-Null",
            exe.to_string_lossy().replace('\'', "''"),
            arguments.replace('\'', "''"),
        );
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x08000000); // CREATE_NO_WINDOW for the waiting helper.
        command
    };

    // A Finder/direct relaunch has no runner owning stdin. Inheriting this opt-in
    // would treat the helper's null stdin as a fresh request to quit immediately.
    command
        .env_remove("VORKSPACE_DEV_RELOAD")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

// ProcessStartInfo.Arguments uses the Windows native command-line convention.
// Start-Process -ArgumentList joins strings and loses empty/quoted arguments.
#[cfg(any(windows, test))]
fn windows_argument(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            result.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n('\\', slashes));
        }
        result.push(ch);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_arguments_preserve_empty_quotes_and_trailing_backslashes() {
        assert_eq!(windows_argument(""), "\"\"");
        assert_eq!(windows_argument("a project"), "\"a project\"");
        assert_eq!(windows_argument("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(windows_argument("C:\\project\\"), "\"C:\\project\\\\\"");
        assert_eq!(windows_argument("a\\\"b"), "\"a\\\\\\\"b\"");
    }
}
