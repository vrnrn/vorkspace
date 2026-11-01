//! Relaunch only after this process exits, including its asynchronous quit hooks.
use std::{
    ffi::OsString,
    io,
    path::Path,
    process::{Command, Stdio},
};

pub fn relaunch_self() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();

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

pub fn wait_and_relaunch(pid: u32, exe: &Path, args: &[OsString]) -> io::Result<()> {
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
