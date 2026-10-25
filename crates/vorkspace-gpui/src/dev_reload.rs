//! The development runner owns stdin; a command or EOF requests normal shutdown.
//! Enabled only by VORKSPACE_DEV_RELOAD=1, independently of the build profile.
use gpui::App;

pub fn install(cx: &mut App) {
    if std::env::var("VORKSPACE_DEV_RELOAD").as_deref() != Ok("1") {
        return;
    }
    let receiver = shutdown_request(std::io::BufReader::new(std::io::stdin()));
    cx.spawn(async move |cx| {
        if receiver.recv().await.is_ok() {
            cx.update(crate::quit);
        }
    })
    .detach();
}

fn shutdown_request(
    mut input: impl std::io::BufRead + Send + 'static,
) -> async_channel::Receiver<()> {
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let mut command = String::new();
        let _ = input.read_line(&mut command);
        let _ = sender.send_blocking(());
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_command_and_eof_both_request_normal_shutdown() {
        for input in ["quit\n", ""] {
            let request = shutdown_request(std::io::Cursor::new(input));
            assert_eq!(request.recv_blocking(), Ok(()));
            assert!(
                request.recv_blocking().is_err(),
                "only one shutdown request"
            );
        }
    }
}
