# Vox Voice Controller (ELI5)

**Vox** is a little voice assistant built into the top-right corner of Vorkspace's title bar. Instead of typing to every chat tab or Bot by hand, you can talk out loud to Vox and ask it to check on your sessions and Bots, start new tasks, or read replies back to you.

For the full implementation guide, see [Vox](../vox.md).

---

## 1. How to Turn It On and Use It

1. Vox is **off by default** so it never touches your microphone unless you want it. Turn it on in **Settings → Vox → General → Enable Vox** (it uses your signed-in OpenAI Codex ChatGPT live voice account).
2. Click the little `vox` face in the top-right corner of the window to start a voice call, and speak naturally:
   - *"Tell Helper to review the latest change."*
   - *"Ask the session called tests to fix its failures."*
   - *"Read the last reply in that session."*
3. While you're on a call, the little ASCII face blinks, thinks, and talks. Click the face again anytime to hang up.

---

## 2. Silent Replies Mode

What if you are in a quiet office or on a train where you want to whisper instructions to Vox, but don't want your computer speaking out loud?
- Turn on **Silent replies** in **Settings → Vox → Audio**.
- Vox mutes its voice immediately (without dropping the call) and shows its latest reply in a small text bubble right underneath its face!

---

## 3. Memory and Continuity

- **One Ongoing Conversation**: Switching between project tabs or opening Settings doesn't hang up your call, and Vox remembers your conversation the next time you call it.
- **Reset Anytime**: If you want a fresh start, click **Reset conversation** in **Settings → Vox → Conversation**.
- **Long-Term Memory**: Vox can also remember helpful facts about how you like to work (**Settings → Vox → Memory**), and those memories stay saved even if you reset the conversation.
