# Vorkspaces

A vorkspace is a named set of projects (folders or repositories) and Bots: the work a window is for. The switcher at the top of the left rail shows the active vorkspace; its menu switches vorkspaces or opens **Manage vorkspaces**, where vorkspaces are created and deleted.

## Model

`workspace::vorkspaces` owns the definitions, stored with the window's saved workspace state:

- `vorkspaces`: each has an id, a name, an ordered list of project paths, a list of Bot ids, and optional icon letters, color and chat background.
- `active_vorkspace`: the vorkspace this window shows.
- `vorkspace_project`: the project each vorkspace last showed, so switching back returns there.
- `seen_bots`: Bots already placed once.

Membership is a view, not ownership. A folder can belong to several vorkspaces and its sessions appear in each. Removing a folder from a vorkspace only hides it there; its sessions and data stay. A Bot belongs to one vorkspace at a time: adding it to another moves it. Bots keep running whichever vorkspace lists them.

On the first launch with vorkspaces, every existing project goes into one vorkspace named **Main**. A project the app has never seen before joins the active vorkspace, however it arrives: Open project, a remote host, or an agent command. A Bot never seen before joins the active vorkspace when the roster loads, so existing Bots also land in Main. A Bot deliberately removed from every vorkspace stays out until it is added back. The last vorkspace cannot be deleted.

## Window

Everything that lists projects uses the active vorkspace's projects: the sidebar header's project picker, Search, the quick composer, app pages such as Notes and Automations, and a Bot's project choices. The rail's Bots section hides Bots placed in another vorkspace.

Switching remembers the current project, makes the target active and opens its last project (or its first) the same way an explicit project pick does: a visible Bot pane gives way to that project's session. A vorkspace with no folders shows the no-project state, including after relaunch. Search and Back or Forward can reach another project; opening its conversation switches to a vorkspace containing that folder when necessary. New sessions use the checkout shown by the focused conversation, when there is one.

The rail no longer lists projects. Its **Sessions** section lists the vorkspace's sessions across its projects: five at a time, working and approval-blocked ones first, then pinned and the latest. The full-width chevron under the list shows every session; the list then scrolls on its own and only renders nearby cards. Navigation and Bots scroll separately when the window is too short to fit them alongside Sessions. The header holds title search, a filter and a new-session button. The filter narrows by archived, status, time, provider, project and Bot (any, one Bot by name, or no Bot); searching or filtering lists every match. Right-click a card or press Shift-F10 on a focused card to open its menu (pin, rename, link, remind, archive, delete), F2 renames, Delete removes a multi-selection, hovering in the compact rail shows a preview to the right after a moment, and dragging a card onto the workspace opens it in a split. Sessions done for an orchestrator's task are not listed on their own. Session folders are gone; nothing in the window shows or creates them. Each card shows the title, status or age, the provider, the project, and the Bot that started it, when one did. Blank sessions are never listed, and archived ones only when the filter includes them. Listing a session never loads its saved history. A Bot's panel opens its launched sessions and documents. Sessions also appear in the rail with the Bot's name when its sidebar setting is enabled.

**Manage vorkspaces…** opens a dialog with the vorkspace list on the left. For the selected vorkspace it edits the name, its **Look** (a color for its mark, and a chat background shown behind every session in it), its folders (remove, **Add folder…**, or add a folder known from another vorkspace) and its Bots (remove, or add one listed elsewhere). It can switch to that vorkspace or delete it, with a confirming second click.

## Customization

Customization belongs to the vorkspace. Under **Look**, **Icon letters** accepts up to two letters. Clearing the field restores the automatic initial from the vorkspace name. Projects show a folder icon in the color its path hashes to, under its folder name; older per-project names, logos, colors and backgrounds are dropped on load. Each project has its own tab in **Manage vorkspaces…**, holding its Codebase Memory and Notifications settings; **Project settings…** opens that tab. The vorkspace's chat background is drawn behind every session while that vorkspace is active.

## Sidebar toggle

The project sidebar (Explorer, Changes, To-dos) is a persistent toggle: the folder button beside the rail toggle in the top bar, or ⇧⌘B. It stays as left over Bot panes, app pages and empty projects, and it is never collapsed automatically to make room for a split; a split is admitted only if it fits beside it. Only Settings hides it while open. Its header has a project picker and a project actions button for settings, Finder, archive and delete. The picker can also switch worktrees. The top bar has no back or forward buttons; ⌘[ and ⌘] still step through visits.

Connector lists show items for the visible vorkspace. Background issue and pull request polling covers registered local projects across vorkspaces, so switching views does not reset the automation event baseline.

The rail's options menu no longer has an Activity switch. The expanded rail lists working sessions first, and the compact rail keeps its strip of working agents.

## Workspace windows

**File → New Window** (⇧⌘N) opens a fresh window in the current vorkspace. To open a specific vorkspace, choose **Open in New Window** from the vorkspace switcher. Each window has its own selected vorkspace, project, tabs, pane layout, and zoom. The app restores open layouts on launch and brings the last active layout to the front. Closing one window removes it from the restore list. The last layout remains available on the next launch.

`shell::app_core::AppCore` is the application's single owner of what windows share, one per data store:

- Vorkspace definitions and Bot membership are saved under the `vorkspaces` setting. The shared roster commits each Bot and its backing session before a window can run that conversation.
- The `workspace-windows` setting holds window layouts, project availability, and shared preferences. The focused layout also feeds the older `workspace` setting for compatibility.
- The scheduler lease lets one window fire reminders and automations. The shared Bot roster also gives one manager responsibility for Habit scheduling.
- A conversation has one live owner. Opening it in another window raises its owner. Once the owner closes, another window can load the saved conversation. Session metadata changes reach the other windows without opening their transcripts.
- Remote-host connections, the native Dock badge, and the agent app bridge are shared. The bridge routes a call to the window running the calling session, falling back to the oldest window.

**Window → Close Window** (⇧⌘W) checks for active work and prompts before discarding unsaved file edits. Notes must finish saving before the window closes. The last window honors **Close to Tray** when enabled.

Note editors compare each save with the version they loaded. If the note changed elsewhere, the editor keeps its draft and offers **Save a Copy** or **Discard and Reload**.
