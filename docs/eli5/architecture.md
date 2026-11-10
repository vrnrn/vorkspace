# Architecture

The desktop app window is drawn by GPUI. Rust keeps track of conversations, files, Bots, and running agents in the same desktop process. The code is split into several crates, which are smaller building blocks with one job each, such as colors and animation timing or understanding code in the editor. Splitting them changes how the code is organized, not how the app runs: they are all still one app. Saved history lives in SQLite, with extra window settings beside it.

The app reads the saved workspace and Bot list once as it opens. Windows share the same conversation list, including names, draft markers, and working folders. Each window keeps its own tabs and selection. If a conversation is already open in another window, the app takes you to that window.

All open windows share the same reminders and linked GitHub updates. Dismissing an update in one window clears it everywhere. Opening or snoozing an old reminder leaves any newer reminder you scheduled in place.

Stopping an orchestration also stops its prepared workers from opening new conversations. Workers that already started keep the conversation and work they saved.

A Bot's face and name stay at the top of its chat as messages scroll. Its face shows whether it is idle, working or needs you, without following the mouse. The app draws a Bot's body picture once and then moves only its eyes, so blinking and looking around are cheap drawing rather than a fresh picture every frame. Idle faces now and then glance around or blink, all at the same moments, then hold still, so a quiet chat redraws the window only a few times every few seconds instead of thirty times a second. Each window’s sidebar faces and its settings preview follow the mouse when it moves; working and attention faces keep animating. Old face pictures stay in graphics memory while a window still uses them and are removed afterward. On Mac, messages fade gently around the face to reveal the chat's own background colors and picture. You can still select, click and scroll the chat there, and the fade stays above the typing box. Hidden or covered faces stop animating, and an empty chat shows help instead of a second face.

Picking a project from the top of the sidebar always takes you to that project's sessions, even if you were looking at one of its Bots. The full sidebar starts with your [vorkspace](vorkspaces.md) and lists that vorkspace's sessions, running ones first. In the narrow icon sidebar, the Working card appears above Subscriptions when a session is running, needs approval, or has finished while you were looking elsewhere. It covers sessions across projects; click one to open it. The list starts with four sessions; More opens a scrollable list and Show less returns to four. Each session shows its name, what tool it last used (when known), its project and elapsed time. The card disappears when nothing qualifies, and it still appears for just one session. In a short window, scroll the upper sidebar to reach navigation; the card and bottom controls stay available. You can turn off Working agents in General settings.

Each new worktree can run a setup command you choose under **Settings → Worktrees**, before its first agent turn. Pressing Stop ends a running setup. If setup fails or gets interrupted, the app keeps the worktree and includes a warning in agent requests until one finishes. An agent can see the same warning again if a turn ends early or two sessions share the worktree. Settings shows which worktrees passed the removal checks and how much disk they use. If Git cannot verify a check, the app keeps the worktree during agent cleanup.

Closing the window keeps the app running in the menu bar, and clicking its Dock icon brings the window back.

Changing a setting or choosing a model updates the shared preferences once. Other windows pick up the change and keep their own zoom. If two windows change different settings, both choices are saved.

Open Bots in the main app window. Click the macOS menu bar icon or press ⌘⇧C to find recently copied text and images. Search finds text; image thumbnails show what you copied. Hover over an image for one second to show its larger preview; moving away hides that pane. Choosing an item puts its original contents back on the clipboard, ready for your usual Paste command. New meeting and Settings are also in this menu. New meeting opens a blank note with recording controls. During recording, Stop recording replaces New meeting. Quick Composer still has its global shortcut.

Clipboard history keeps at most 50 items in memory. A small helper app keeps it running after you quit vorkspace, so copies keep being saved and ⌘⇧C keeps working. History disappears when you choose Quit in the clipboard menu, turn it off, log out, or update the app. After logging out, it starts again the next time you open vorkspace. When clipboard history and dictation are off and no note is recording, clicking the app's menu bar icon brings the window back. Clipboard history also has a memory limit, so large images can make older items leave sooner. Very large copies are skipped. Clear history removes the app's saved copies and leaves your current system clipboard alone. Turning clipboard history off in the Clipboard group in General settings clears it and stops watching for copies. When history is off, the helper’s menu shows New meeting (or Stop recording), Settings and Quit. Search looks flat until you click or begin typing in it. Password-manager copies marked private are ignored.

The app checks whether the clipboard changed twice a second. It makes small image previews in the background, one at a time, and never saves a clipboard history file.

Rust controls Vox calls and translates desktop Copilot requests. Small JavaScript helpers handle editor language tools, voice audio, and remote machines. The editor helpers get a limited amount of working time, and time spent waiting while other apps keep the computer busy does not count against it. Empty conversations show the composer, and session titles stay still. A remote machine keeps its own agents running and history saved even when your desktop disconnects. Notes, recordings and their summaries are drawn by the app itself. Vox audio and HTML previews use separate browser surfaces, and HTML previews cannot send commands to the app.

Notes, Automations, connector pages, Todos and Search share [one compact design](native-pages.md). The same controls use the same sizes and colors. Narrow windows keep navigation usable without throwing away the selected item or unfinished text.

The [repository map](repository.md) shows where each part lives. Embedded helper code has its own folder; the scripts folder contains developer tools.

The app reads saved messages and details about their replies together. Switching back to a window refreshes the chats you can see. Hidden chats with drafts or running work keep the history they need. An oversized hidden chat can free its memory while smaller chats stay ready to reopen. Repeated terminal redraw requests are combined. Cleaning up one idle agent leaves the shared lock available to other agents.

The app groups quick bursts of command output and keeps a readable tail. An idle agent can stay ready for the next message while the saved transcript stays in the conversation's storage.

The app keeps word-fade animation data while new text is arriving, then releases it. When you scroll a long chat, the prompt rail uses your position to find nearby prompts.

The app prepares a few attachment thumbnails at a time across all windows. Opening a picture full screen has its own loading path.

The Changes panel and title bar read from the same Git snapshot. On your Mac, the app listens for file changes in an active checkout and still does a periodic check. Remote checkouts and folders the Mac cannot watch use periodic checks. Two windows on the same checkout share a commit graph, and each window keeps its own search and selection. If you switch checkouts while writing a commit message, each checkout keeps its own draft while Changes is open. A confirmation belongs to the checkout and branch where you opened it.

Open **View → Running Work** to see the app’s local agents. A Stop click is tied to the reply shown in the list, even from another window. The app waits for a reply's final save before starting another reply in the same conversation.

When a Bot receives a message, the app sends that message's attachments and options with its own reply. Your composer and chat settings stay ready for your next message.

Side questions keep their own replies and unfinished drafts. You can switch between them or close the sheet and return later. A side question can read the conversation it came from and runs separately from the main reply.

When the app opens one saved remote conversation, it looks up that conversation's saved information directly. It refreshes the lookup when remote bindings change and keeps older tab identities and unsent messages.

Long indexed Bot chats opened from the Bot panel first show their newest messages. Scroll up or choose Load earlier messages to read older parts without losing your place. If the saved chat changes elsewhere, the app reloads it when you request another page; if loading fails temporarily, you can retry. Before a Bot runs or a message changes, the app loads the complete chat so older messages stay safe. Other kinds of history keep their existing loading behavior.

Bots, sessions, terminals, notes, GitHub/GitLab/Jira items, Search, Automations and To-dos open in tabs. Different notes and items can stay open independently. Opening the same item again takes you back to its existing tab and unfinished work.

Click a project to return to its workspace.

Files opened from Search or a Bot link stay with the working copy they came from. A new file pane opens beside the Bot when there is room, or in its own tab. Closing and reopening the file tab keeps unfinished edits while the app is open. Add to chat puts your comment in the original Bot's draft, even if you've moved the file or restarted. If the chat can't accept it, your comment stays available to copy or try again. GitHub activity appears in the Bot chat it belongs to.

Drag a tab onto another pane’s edge to put them beside or above one another. Drag a pane’s small header to move it, or back to the tab bar to give it its own tab. You can resize the space between panes. The same limits apply throughout: each column needs 420 points of width, and up to three content panes can be stacked vertically. Existing file-pane exceptions stay the same. When you shrink the window, an older large layout scrolls instead of throwing away panes.

Click a document in a Bot’s floating island to open it beside the Bot. If there is not enough room, it opens in its own tab. The island stays inside that Bot’s pane. On narrow panes, click the Bot face or the small button at the top right to open it. The island’s Bot settings button opens settings, including Habits. Bot diffs are not shown. Closing a Bot tab keeps its conversation, unsent draft and running work. Your tab order, layouts and selected panes are remembered after restarting.

Corners follow the same size-based pattern throughout the app: small controls have tighter curves, while cards, menus and floating panels have softer corners. Highlights and borders follow the shape of the item they belong to. Native Mac window corners, circular buttons and Bot faces keep their own shapes.

Sidebar rows and top tabs have simple rounded highlights when hovered or selected. Those highlights stay separate from the curved pane edges. Explorer, Changes and To-dos sit beside the rail on the left; your sessions are listed in the rail itself. When the rail is folded down to icons it keeps the same order: your vorkspace, the navigation icons, every Bot, and a Sessions button that unfolds the rail. The compact rail and top navigation use one continuous background. [Project to-dos](todos.md) work on their own without GitHub; linking them to GitHub issues is optional. Bots use those project boards too.

Hold Command and click the **+** in a project's tab bar to open a new terminal, starting in your selected project or worktree folder. A plain click, or ⌘T, starts a session instead. You can keep several independent shells in the same folder. The [Ghostty terminal engine](terminal.md) understands their output and keyboard input; Vorkspace draws the terminal inside the app. Click a terminal tab at the top to return to that exact shell. Switching tabs keeps its output and commands running; selecting a different folder never moves an existing shell. Closing its tab ends only that shell, with a warning if a command is running. After quitting and reopening the app, saved terminal tabs start new shells in their saved starting folders when selected. Running programs and old terminal output do not survive quitting. The smaller terminal dock and its shortcuts still work separately.

As a reply arrives, only changed chat blocks are saved. The screen can show a small preview while the full tool result stays safely in the database. A stale window cannot overwrite newer saved history.

Pending remote requests and their attachments stay saved on your Mac. The remote machine can report new progress while those requests are waiting for confirmation or Retry.

Bot cards save in the background, one card at a time. Repeated updates combine, and an unchanged card needs no database work. Saving a card leaves the rest of the conversation untouched. The screen changes after the save succeeds, provided it still shows the same card and conversation. An old reply cannot replace a newer card. Saving a later reply also cannot put an older saved card back. Closing its view does not cancel a save. Quitting gives pending saves the app's usual short cleanup window, so busy storage cannot hold Restart open indefinitely. If the app exits before a card refresh saves, its previous saved value remains and can be refreshed next time.

Every supported agent has an output size limit before it is buffered. A fast agent waits when the app is still handling earlier output. An individual response that is too large produces a clear error rather than silently losing part of its text. Stopping the connection also releases waiting readers.

Interface zoom makes the whole workspace larger or smaller, from 60% to 160%. Clicking, typing, native previews, and screen-reader controls follow the same zoom level, including after moving the window to a different display.

Panels reuse saved drawing work while nothing changes. When a sidebar starts opening or closing, the app checks that saved drawing positions still match the last frame and redraws any that are out of date. Your selection and unfinished work stay in place.

The app works with VoiceOver and the keyboard. VoiceOver says when a session finishes or needs you, Tab reaches tabs and the main buttons, and spinners stop when Reduce Motion is on.

Each Mac window remembers which mouse pointer it should show. It updates the pointer straight away and puts it back if macOS resets it. Web and video panes are drawn by macOS itself, so whenever one moves, hides, or appears, the window recalculates where its own pointer rules apply.

The background app is named **vorkspace helper**. It remains available after the main app quits while clipboard history or dictation is enabled. It also runs during meeting recordings and sends recording updates and transcript text to the main app. Clipboard history stays in the helper.

Optional [Codebase Memory](codebase.md) uses a local helper engine to map a project’s symbols and calls. The app decides which projects may use it, keeps each checkout separate, and stops the work when you disable it.
