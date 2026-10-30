# Minions

Minions are an experimental way for a Bot to split work. Turn them on with **Settings › Bots › Minions**; they are off by default and require Bots.

## Spawning

With Minions on, a Bot's context explains `minions.spawn {prompt,name?,project?,harness?,model?}` and asks it to use minions whenever the user mentions a minion or minions, and otherwise when parallel parts of a task benefit. Spawning follows the `sessions.start` path: it creates a session in one of the Bot's assigned projects (its first project by default), inherits the Bot's execution settings, and queues the prompt immediately. Its title is the minion's name, or the next free "Minion N". The start is recorded as a launch with completion reporting on, so the parent receives one report when its minions stop. `minions.list` returns each minion's name, session, project and whether it is working. The parent messages a minion with `sessions.send`.

## Identity and shared memory

A minion is recorded on its parent (`Bot.minions`: name, session, project, creation time) in the roster. Its context and capability are never saved: `Bots` rebuilds a minion registry from the roster on load, save and preference changes, and the shell attaches each entry to the minion's conversation view when the view is created, or when Bots starts for views opened earlier. The registry is shared state rather than an entity read, so attaching never borrows `Bots` while it may be mid-update. A new minion is recorded before its view is created and withdrawn if the start fails, so its first turn already carries the attachment.

On every turn the attachment supplies `Settings.minion` and the Bot bridge's environment for that session, instead of the session's own operator environment. The runtime adds the minion context, built from the parent's soul and current memory, whenever it changes, using the same per-session context receipt as Bot chats. A minion is an ordinary project session otherwise: it does not use Bot chat delivery, outbox or composer behaviour.

The Bot host resolves a minion's app calls to its parent, so `memory.*` reads and writes the parent's memory, shared by the parent and all of its minions, and `todos.*` uses the parent's board. Minions may use `memory.*`, `todos.*`, `notes.*`, `soul.read`, `sessions.list/read`, `worktrees.list`, `minions.list`, `artifacts.list/read` and `models.list`. They cannot start or control sessions, spawn minions, or change the parent's soul or habits.

## Sidebar

Minion sessions always show under their project like other sessions, even when the parent hides the sessions it starts, with a small badge naming the parent.
