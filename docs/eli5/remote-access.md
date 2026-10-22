# Remote Access Over SSH (ELI5)

What if your laptop is small or slow, and your real code lives on a powerful desktop, a cloud server, or a Mac mini in another room?

**Remote Access** lets you use Vorkspace on your laptop while the AI agents, files, and Git branches actually run on that other computer over **SSH**.

For the full technical guide and manual commands, see [Remote access](../remote-access.md).

---

## 1. How It Works in Plain English

1. In **Settings → Connections → Add machine**, you type the SSH address of your other computer (like `me@my-server`).
2. Vorkspace logs in over SSH, downloads a small helper program (`vorkspace-host`) onto that computer, checks its security fingerprint (SHA-256), and starts it as a background service.
3. Vorkspace opens a private tunnel through SSH (`127.0.0.1:3774`) so no outsider on the network can talk to your helper.

---

## 2. Why It's Awesome

- **Close Your Laptop Anytime**: Because the AI agents run on the remote computer, closing your laptop lid or losing Wi-Fi **does not stop your agents**. When you open your laptop again, Vorkspace reconnects and shows everything the agent did while you were away.
- **No Double Sends**: Every message you send has a unique ticket number. If your Wi-Fi blinks right as you hit Send, clicking **Retry** checks if the remote computer already got that ticket instead of sending the message twice.
- **Feels Just Like Local**: You can browse remote files, edit code, view Git diffs, switch branches, create worktrees, and attach images right from your laptop.

---

## 3. Removing or Uninstalling a Remote Machine

When you click **Remove** on a connection in Settings, you get two choices:
- **Remove from this desktop only**: Forgets the machine on your laptop, but leaves the remote computer and its agents running.
- **Revoke access and remove**: Tells the remote computer to cancel your laptop's digital key first, then removes the connection.
If you ever want to completely remove the background helper from the remote computer, run `vorkspace-host service uninstall` on that machine.

The remote host is maintained in JavaScript and includes Node in packaged archives. Building an archive does not publish it; automatic SSH installation needs archives for the exact desktop version.

The source for the remote host lives under `host/`. Its connection test is `scripts/checks/test-remote-ssh.py` and uses a temporary local server.

Checking whether a remote chat changed reads its small summary instead of reopening the whole chat. Large downloads arrive in pieces. If a download is abandoned, the host releases its temporary copy after two minutes without activity.

Temporary downloads share a size budget. If it is busy, retry after another download finishes; active downloads are kept intact. A chat that exceeds the download size limit gets a clear error and stays saved on the host. Download limits do not delete conversation data.

A slow reply from the host cannot undo newer status or settings. Partial chat lists can be filled in by a matching complete reply without treating missing fields as deleted.
