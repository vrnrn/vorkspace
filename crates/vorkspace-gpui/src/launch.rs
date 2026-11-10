//! Resolve the invocation and prepare desktop state before GPUI starts.
use crate::{
    fixtures,
    shell::{PreparedCore, WindowLayouts},
    workspace::Workspace,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};
use vorkspace_core::{
    store::{Session, SessionSidebarMetadata, Store},
    transcript::Transcript,
};
use vorkspace_runtime::runtime::Database;

const HELP: &str =
    "vorkspace-gpui [--cwd PATH] [--dev-data | --vorkspace-data | --vorkspace-db PATH]
All build profiles use the existing desktop database by default.
--dev-data opens an isolated development database using vorkspace's existing schema.
--vorkspace-data opens the existing desktop vorkspace.db directly.
--vorkspace-db PATH opens a specific desktop database.
--fixture, --chat-fixture and --visual-fixture add local samples to isolated development data only.
visual docs prints the native visual SDK reference.
visual check/show --input FILE validates or publishes native session visuals.
visual-mcp runs the session visual tool server over standard input/output.
--check-data validates and lists the selected store read-only, then exits.";

#[derive(Debug, PartialEq, Eq)]
enum DataSource {
    Desktop(Option<PathBuf>),
    Development,
}

#[derive(Debug, Default)]
struct Samples {
    long: bool,
    chat: bool,
    visual: bool,
}

#[derive(Debug)]
struct Options {
    source: DataSource,
    cwd: Option<PathBuf>,
    check_data: bool,
    samples: Samples,
}

impl Options {
    fn parse(
        mut args: impl Iterator<Item = String>,
        mut desktop: bool,
    ) -> Result<Option<Self>, String> {
        let mut desktop_db = None;
        let mut cwd = None;
        let mut check_data = false;
        let mut samples = Samples::default();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fixture" => samples.long = true,
                "--chat-fixture" => samples.chat = true,
                "--visual-fixture" => samples.visual = true,
                "--dev-data" => desktop = false,
                "--vorkspace-data" => desktop = true,
                "--vorkspace-db" => {
                    desktop = true;
                    desktop_db = Some(args.next().ok_or("--vorkspace-db needs a path")?.into());
                }
                "--check-data" => check_data = true,
                "--cwd" => cwd = Some(args.next().ok_or("--cwd needs a path")?.into()),
                "--help" | "-h" => return Ok(None),
                _ => return Err(format!("Unknown option: {arg}")),
            }
        }
        if desktop && samples.requested() {
            return Err(
                "Fixtures require --dev-data; they cannot be written into your vorkspace database."
                    .into(),
            );
        }
        Ok(Some(Self {
            source: if desktop {
                DataSource::Desktop(desktop_db)
            } else {
                DataSource::Development
            },
            cwd,
            check_data,
            samples,
        }))
    }
}

impl DataSource {
    fn database(&self) -> Result<PathBuf, String> {
        match self {
            Self::Desktop(path) => Ok(path
                .clone()
                .unwrap_or_else(vorkspace_core::store::vorkspace_database)),
            Self::Development => {
                let database = development_dir().join("vorkspace.db");
                if same_database(&database, &vorkspace_core::store::vorkspace_database()) {
                    return Err("The isolated development directory points at the desktop database. Choose a separate VORKSPACE_NATIVE_DATA_DIR.".into());
                }
                Ok(database)
            }
        }
    }
}

fn same_database(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata(left)
            .ok()
            .zip(std::fs::metadata(right).ok())
            .is_some_and(|(left, right)| left.dev() == right.dev() && left.ino() == right.ino())
        {
            return true;
        }
    }
    database_location(left)
        .zip(database_location(right))
        .is_some_and(|(left, right)| left == right)
}

/// Resolve the existing part of a path even before its database is created.
fn database_location(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let (ancestor, mut location) = absolute.ancestors().find_map(|ancestor| {
        ancestor
            .canonicalize()
            .ok()
            .map(|resolved| (ancestor, resolved))
    })?;
    for component in absolute.strip_prefix(ancestor).ok()?.components() {
        match component {
            std::path::Component::Normal(name) => location.push(name),
            std::path::Component::ParentDir => {
                location.pop();
            }
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(location)
}

impl Samples {
    fn requested(&self) -> bool {
        self.long || self.chat || self.visual
    }

    fn seed(&self, db: &mut Store, cwd: &str) -> Result<(), String> {
        let mut save = |title, history: Transcript| {
            let session = db.create("codex", cwd)?;
            db.title(session.id, title)?;
            db.write_rows(session.id, 0, &history.rows)
        };
        if self.long {
            let mut history = Transcript::default();
            let started = Instant::now();
            for i in 0..10_000 {
                history.begin(if i % 2 == 0 { "user" } else { "assistant" });
                history.append(&format!("Message {i:05} · Native transcript stress fixture.\nUnicode stays intact: 🙂 漢字 café. Scroll up while another session streams.\nThis message is local fixture data; sending resumes a new real provider conversation."));
            }
            let count = history.rows.len();
            save("Scroll lab · 10,000 messages", history)?;
            eprintln!("Seeded {count} rows in {:?}", started.elapsed());
        }
        if self.chat {
            save("Native chat · Markdown and tools", fixtures::chat())?;
        }
        if self.visual {
            save(
                "Native visuals · Controls, charts and 3D",
                fixtures::visuals(),
            )?;
        }
        Ok(())
    }
}

pub(crate) struct Startup {
    pub db: Database,
    pub sessions: Vec<Session>,
    pub metadata: SessionSidebarMetadata,
    pub core: PreparedCore,
    pub saved: Option<String>,
    pub cwd: String,
    pub explicit_cwd: bool,
    pub diagnostics_path: Option<PathBuf>,
}

pub(crate) fn prepare() -> Option<Startup> {
    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        Some("visual-mcp") => {
            vorkspace_core::visual_bridge::mcp_stdio().unwrap_or_else(|error| fail(&error, 1));
            return None;
        }
        Some("visual" | "app") => {
            let visual = args.next().as_deref() == Some("visual");
            let rest = args.collect::<Vec<_>>();
            let result = if visual {
                vorkspace_core::visual_bridge::client(&rest)
            } else {
                vorkspace_core::bot::bridge::client(&rest)
            };
            let value =
                result.unwrap_or_else(|error| serde_json::json!({"ok":false,"error":error}));
            println!("{value}");
            if value["ok"] == false {
                std::process::exit(1);
            }
            return None;
        }
        _ => {}
    }

    let Some(options) = Options::parse(args, bundle_uses_desktop_data().unwrap_or(true))
        .unwrap_or_else(|error| fail(&error, 2))
    else {
        println!("{HELP}");
        return None;
    };
    let database = options
        .source
        .database()
        .unwrap_or_else(|error| fail(&error, 2));
    let db = if options.check_data {
        Store::read_vorkspace(&database)
    } else {
        Store::open_vorkspace(&database)
    }
    .unwrap_or_else(|error| fail(&format!("Cannot open {}: {error}", database.display()), 1));
    if options.check_data {
        let sessions = db.sessions().unwrap_or_else(|error| fail(&error, 1));
        eprintln!(
            "Database: {}\n{} saved sessions; existing vorkspace schema; no import or migration.",
            database.display(),
            sessions.len()
        );
        return None;
    }
    Some(prepare_desktop(db, &database, options).unwrap_or_else(|error| fail(&error, 1)))
}

fn prepare_desktop(mut db: Store, database: &Path, options: Options) -> Result<Startup, String> {
    db.start_attachment_cache()
        .map_err(|error| format!("Cannot prepare attachment storage: {error}"))?;
    db.recover_local_submissions()
        .map_err(|error| format!("Cannot recover pending submissions: {error}"))?;
    vorkspace_core::checkpoint::init(
        database
            .parent()
            .unwrap_or(Path::new("."))
            .join("checkpoints"),
    );
    let saved = db.ui_state().unwrap_or_else(|error| {
        eprintln!("Cannot read workspace settings: {error}");
        None
    });
    let layouts = WindowLayouts::load_with_legacy(&db, saved.as_deref());
    let core = PreparedCore::read(&db, layouts);
    let mut sessions = db.sessions()?;
    let explicit_cwd = options.cwd.is_some();
    let cwd = launch_cwd(
        options.cwd,
        matches!(options.source, DataSource::Desktop(_)),
        saved.as_deref(),
        &sessions,
    );
    if options.samples.requested() {
        options.samples.seed(&mut db, &cwd)?;
        sessions = db.sessions()?;
    }
    if sessions.is_empty() {
        sessions.push(db.create("codex", &cwd)?);
    }
    let workspace = Workspace::restore_sessions(saved.as_deref(), &sessions, &cwd);
    let mut projects = workspace.projects;
    if !workspace.project.is_empty() && !projects.contains(&workspace.project) {
        projects.push(workspace.project);
    }
    Ok(Startup {
        metadata: db.session_sidebar_metadata(&projects),
        db: Arc::new(Mutex::new(db)),
        sessions,
        core,
        saved,
        cwd,
        explicit_cwd,
        diagnostics_path: std::path::absolute(database)
            .ok()
            .and_then(|path| path.parent().map(|root| root.join("diagnostics"))),
    })
}

fn launch_cwd(
    explicit: Option<PathBuf>,
    desktop: bool,
    saved: Option<&str>,
    sessions: &[Session],
) -> String {
    let specified = explicit.is_some();
    if !specified && desktop {
        let saved_project = saved
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|state| {
                state["project"]
                    .as_str()
                    // An empty saved vorkspace deliberately has no launch project.
                    .filter(|project| {
                        !project.is_empty()
                            || state["active_vorkspace"]
                                .as_str()
                                .is_some_and(|id| !id.is_empty())
                    })
                    .map(str::to_owned)
            });
        if let Some(project) = saved_project.or_else(|| {
            sessions
                .iter()
                .find(|session| !session.archived)
                .map(|session| session.cwd.clone())
        }) {
            return project;
        }
    }
    let cwd =
        explicit.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let root = cwd.parent().is_none();
    let cwd = cwd.to_string_lossy().into_owned();
    // Finder and the Dock launch from `/`. Fall back to a project or the home directory.
    if specified || !root {
        return cwd;
    }
    sessions
        .iter()
        .find(|session| session.cwd != cwd)
        .map(|session| session.cwd.clone())
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or(cwd))
}

fn fail(error: &str, code: i32) -> ! {
    eprintln!("{error}");
    std::process::exit(code);
}

fn development_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("VORKSPACE_NATIVE_DATA_DIR") {
        return path.into();
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/vorkspace Native Development")
}

// The bundle runs this binary directly so macOS can admit its menu bar item.
// Info.plist selects the database mode that a launcher flag would have carried.
fn bundle_uses_desktop_data() -> Option<bool> {
    let exe = std::env::current_exe().ok()?;
    let plist = std::fs::read_to_string(exe.parent()?.parent()?.join("Info.plist")).ok()?;
    plist_data_mode(&plist)
}

fn plist_data_mode(plist: &str) -> Option<bool> {
    let rest = plist.split("<key>VorkspaceDataMode</key>").nth(1)?;
    let mode = rest
        .trim_start()
        .strip_prefix("<string>")?
        .split('<')
        .next()?;
    match mode {
        "vorkspace-data" => Some(true),
        "dev-data" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_plist_selects_the_data_mode() {
        let plist = |mode: &str| {
            format!("<dict>\n<key>VorkspaceDataMode</key><string>{mode}</string>\n</dict>")
        };
        assert_eq!(plist_data_mode(&plist("vorkspace-data")), Some(true));
        assert_eq!(plist_data_mode(&plist("dev-data")), Some(false));
        assert_eq!(plist_data_mode(&plist("other")), None);
        assert_eq!(plist_data_mode("<dict></dict>"), None);
    }

    #[test]
    fn resolves_the_last_data_flag_and_rejects_missing_paths_and_desktop_samples() {
        let parse = |args: &[&str]| Options::parse(args.iter().map(|arg| (*arg).to_owned()), true);
        let args = [
            "--vorkspace-db",
            "/tmp/custom.db",
            "--dev-data",
            "--vorkspace-data",
        ];
        assert_eq!(
            parse(&args).unwrap().unwrap().source,
            DataSource::Desktop(Some("/tmp/custom.db".into()))
        );
        assert_eq!(parse(&["--cwd"]).unwrap_err(), "--cwd needs a path");
        assert!(parse(&["--fixture"]).is_err());
        let isolated = parse(&["--dev-data", "--fixture"]).unwrap().unwrap();
        assert_eq!(isolated.source, DataSource::Development);
        assert!(isolated.samples.requested());
    }

    #[cfg(unix)]
    #[test]
    fn development_paths_reject_aliases_before_and_after_database_creation() {
        let dir = tempfile::tempdir().unwrap();
        let desktop = dir.path().join("desktop");
        let development = dir.path().join("development");
        std::fs::create_dir(&desktop).unwrap();
        std::os::unix::fs::symlink(&desktop, &development).unwrap();
        let db = desktop.join("vorkspace.db");
        assert!(same_database(&db, &development.join("vorkspace.db")));
        assert!(same_database(&db, &desktop.join("missing/../vorkspace.db")));
        assert!(!same_database(
            &db,
            &dir.path().join("isolated/vorkspace.db")
        ));
        std::fs::write(&db, "fixture").unwrap();
        let alias = dir.path().join("hardlink.db");
        std::fs::hard_link(&db, &alias).unwrap();
        assert!(same_database(&db, &alias));
    }
}
