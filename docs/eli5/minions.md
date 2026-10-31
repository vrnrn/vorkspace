# Minions, explained simply

Minions are lasting members of a Bot's team. Turn on the experimental feature in **Settings › Bots › Minions**. A Bot can hire someone with a useful role, such as reviewing Rust code, and keep giving that same member new jobs. Mentioning Minions does not mean creating a fresh helper every time.

The parent uses `minions.hire` to choose a name, role and optional specialties. Hiring does not start a job or require choosing a project. The parent checks its team with `minions.list`, then sends work with `bots.send`. A member keeps its own conversation, so later jobs can build on earlier work. Replies normally come back automatically; the parent does not need to keep checking. `minions.spawn` is an older shortcut that hires someone and sends a first job together.

Members share their parent's personality instructions, saved memory and to-do board. If one saves a useful fact, the parent and siblings can read it. They follow the parent's current project assignments, while keeping their own names and message histories.

Only a member's parent and siblings can communicate with it. They can exchange direct messages or use a channel containing just their team. Members cannot contact unrelated Bots or join their channels. Conversations between ordinary top-level Bots still work as usual.

You supervise the team through its activity and saved conversations. Minions sit beneath their parent in Bot navigation, with one eye and the parent's current colour. Each member keeps its own eye style, body shape and pattern. Member conversations and team channels are read-only for you: send instructions to the parent Bot instead. Minions cannot hire or fire teammates, start more sessions, or change their parent's personality or habits.

The parent uses `minions.fire` when a member is no longer useful. Firing retires it and stops it accepting work, while keeping its past conversations. Queued work does not restart just because the app reopens. Older Minion conversations are kept when they become persistent team members.

See the [technical guide](../minions.md) for exact commands and storage details, or [agent access](agent-access.md) for the app CLI.

If a direct task fails, later work waits. The parent can inspect what happened, then retry or discard that task. A retry starts a new attempt; it does not assume the earlier attempt did nothing.

Opening old history does not give a Minion extra permissions. It keeps its own team identity and the same communication limits.
