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
    let Some(bundle) = running_bundle(&exe) else {
        return wait_and_relaunch(std::process::id(), &exe, &args);
    };
    let mut open_args = vec![OsString::from("-n"), bundle.as_os_str().to_owned()];
    if !args.is_empty() {
        open_args.push(OsString::from("--args"));
        open_args.extend(args);
    }
    wait_and_relaunch(std::process::id(), Path::new("/usr/bin/open"), &open_args)
}

/// Restart keeps the running code. A development bundle must not turn into
/// whichever older copy happens to be installed in /Applications.
fn running_bundle(exe: &Path) -> Option<&Path> {
    exe.parent()?
        .parent()?
        .parent()
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
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

#[cfg(test)]
mod tests {
    use super::running_bundle;
    use std::path::Path;

    #[test]
    fn restart_stays_on_the_bundle_that_is_running() {
        for bundle in [
            "/repo/target/native-dev/vorkspace.app",
            "/Applications/vorkspace.app",
        ] {
            let exe = Path::new(bundle).join("Contents/MacOS/vorkspace-gpui");
            assert_eq!(running_bundle(&exe), Some(Path::new(bundle)));
        }
        assert_eq!(
            running_bundle(Path::new("/repo/target/native-dev/vorkspace-gpui")),
            None
        );
    }
}
