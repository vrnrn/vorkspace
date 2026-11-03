fn main() {
    if let Err(error) = vorkspace_meetings_plugin::run() {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
