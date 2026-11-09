# Vorkspaces

A vorkspace is a named set of projects (folders or repositories) and Bots: the work a window is for. The switcher at the top of the left rail shows the active vorkspace; its menu switches vorkspaces or opens **Manage vorkspaces**, where vorkspaces are created and deleted.

## Model

`workspace::vorkspaces` owns the definitions, stored with the window's saved workspace state:

- `vorkspaces`: each has an id, a name, an ordered list of project paths, a list of Bot ids, an optional color and an optional chat background.
- `active_vorkspace`: the vorkspace this window shows.
- `vorkspace_project`: the project each vorkspace last showed, so switching back returns there.
- `seen_bots`: Bots already placed once.

Membership is a view, not ownership. A folder can belong to several vorkspaces and its sessions appear in each. Removing a folder from a vorkspace only hides it there; its sessions and data stay. A Bot belongs to one vorkspace at a time: adding it to another moves it. Bots keep running whichever vorkspace lists them.

On the first launch with vorkspaces, every existing project goes into one vorkspace named **Main**. A project the app has never seen before joins the active vorkspace, however it arrives: Open project, a remote host, or an agent command. A Bot never seen before joins the active vorkspace when the roster loads, so existing Bots also land in Main. A Bot deliberately removed from every vorkspace stays out until it is added back. The last vorkspace cannot be deleted.

## Window

Everything that lists projects uses the active vorkspace's projects: the sidebar header's project picker, Search, the quick composer, app pages such as Notes and Automations, and a Bot's project choices. The rail's Bots section hides Bots placed in another vorkspace.

Switching remembers the current project, makes the target active and opens its last project (or its first) the same way an explicit project pick does: a visible Bot pane gives way to that project's session. A vorkspace with no folders shows the no-project state.

The rail no longer lists projects. Its **Sessions** section lists the vorkspace's sessions across its projects: five at a time, working and approval-blocked ones first, then the latest. The full-width chevron under the list shows every session; the list then takes the rail's remaining height and scrolls on its own while navigation and Bots stay put. The header holds title search, a filter and a new-session button. The filter narrows by archived, status, time, provider, project and Bot (any, one Bot by name, or no Bot); searching or filtering lists every match. Each card has the session's right-click menu (pin, rename, link, remind, archive, delete), F2 renames, Delete removes a multi-selection, hovering shows an archive button and, after a moment, a preview to the right, and dragging a card onto the workspace opens it in a split. Sessions done for an orchestrator's task are not listed on their own. Session folders are gone; nothing in the window shows or creates them. Each card shows the title, status or age, the provider, the project, and the Bot that started it, when one did. Blank sessions are never listed, and archived ones only when the filter includes them. Listing a session never loads its saved history. A Bot's floating island now shows only its documents; the sessions it starts appear in the rail with its name.

**Manage vorkspaces…** opens a dialog with the vorkspace list on the left. For the selected vorkspace it edits the name, its **Look** (a color for its mark, and a chat background shown behind every session in it), its folders (remove, **Add folder…**, or add a folder known from another vorkspace) and its Bots (remove, or add one listed elsewhere). It can switch to that vorkspace or delete it, with a confirming second click.

## Customization

Customization belongs to the vorkspace. Projects have none: each shows as a folder icon in the color its path hashes to, under its folder name, and any older per-project name, logo, colors or background is dropped on load. Each of the vorkspace's projects has its own tab under the vorkspace in **Manage vorkspaces…**, holding its Codebase Memory and Notifications settings; a project's **Project settings…** menu item opens that tab. The vorkspace's chat background is drawn behind every session while that vorkspace is active.

## Sidebar toggle

The project sidebar (Explorer, Changes, To-dos) is a persistent toggle: the folder button beside the rail toggle in the top bar, or ⇧⌘B. It stays as left over Bot panes, app pages and empty projects, and it is never collapsed automatically to make room for a split; a split is admitted only if it fits beside it. Only Settings hides it while open. Its header is just the project name, which opens the vorkspace's project picker, with a row below it for switching the worktree Explorer and Changes show. The top bar has no back or forward buttons; ⌘[ and ⌘] still step through visits.

The rail's options menu no longer has an Activity switch. The expanded rail lists working sessions first, and the compact rail keeps its strip of working agents.

## One window per vorkspace

Vorkspaces are designed so each can own a window. Separate windows are being built in steps, so that nothing runs twice and no session has two owners.

`shell::app_core::AppCore` is the application's single owner of what windows share, one per data store:

- Vorkspace definitions and placed Bots, saved under the `vorkspaces` setting. A change in one window reaches every other window; each window keeps its own choice of active vorkspace.
- The scheduler lease. Only the holding window fires reminders and automations. When it closes, the next window to ask takes the lease.
- Session leases. A session is live (has a `Conversation` and may run a provider) in at most one window. Another window lists it but does not load it. A lease is free once its window closes or no longer has the session loaded.

- The agent app bridge. The core starts its socket once. Each call goes to the window with the calling session live, else the window whose Bots run that session, else the oldest open window.

Leases name their holder and are checked again at every claim, so a closed window never leaves one stuck.

Still owned per window, and the remaining blockers before the app opens a second window: the Bot manager (Habit scheduler, peer outbox and its saved roster), Vox, remote-host connections and OS notifications.
