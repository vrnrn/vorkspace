# vox — expressive title-bar control

## scope and direction contract

This is a code-led Operate refinement of Vorkspace's existing desktop chrome.
The user clarified that “menu bar Vox” means the top-right control inside
Vorkspace. The earlier independent-window experiment remains reverted.

- Thesis: Vox lives in its existing title-bar slot; its three glyphs carry call state and personality.
- Own-world: shared grayscale content, native app glass, existing title-bar fills, monospace glyph artwork, standard Settings casing and spacing.
- Story: click to connect, speak, click again to end. Silent mode is configured in Settings; the latest native reply appears beneath Vox.
- First viewport: one 28-point-high, 56-point-minimum-width face control. A content-sized reply bubble is aligned beneath its right edge, capped at 280 × 180; no microphone or speaker controls remain in the title bar.
- Form: the incumbent title-bar control and shared Popover. Ordinary extension, no new visual world or generated imagery. Signature interaction: static `vox` when stopped, sparse blinks/glances when listening, distinct thinking and talking glyph sequences during a live call.
- Finish: bounded visual inspection, a fresh finish review, updated feature documentation, and a verified local app-only install. The reviewed checkpoint is prepared for an MR; merging and releasing remain separate actions.

## ownership and continuity

- The workspace VoxProvider remains the microphone, WebRTC, and helper owner. Settings injects the same control into its title bar when there is no compact global title bar, avoiding duplicate controls. Replacing or removing title-bar controls does not restart or end a call. Normal workspace hiding/closing and Quit retain existing graceful stop behavior.
- Enable Vox remains the first Settings option, default off, with “Requires Codex with ChatGPT live voice access.” Disabling preserves saved conversation and memory without remounting workspace content.
- Settings → Vox → Audio now persists Silent replies. Changing it updates the current native audio element's `muted` property and applies to subsequent calls and restarts. It does not change microphone input or negotiate another connection.
- VoxButton observes the existing bounded display store. Replies use shared Popover placement and glass, leave focus alone, and can be dismissed by click-away or Escape. The capped bubble scrolls vertically, contains overscroll, and supports keyboard focus with a visible ring without autofocus. Only the latest completed native assistant reply is displayed in silent mode; a later reply can reappear after dismissal. User speech and partial transcripts are not displayed. Underlying case is retained; Vox copy renders lowercase.
- A call that is connecting or live mounts CSS glyph sequences. Stopped calls render only `vox` and mount no animations. Hiding the workspace stops its call; removing the control removes its animations. Reduce Motion uses a static state-specific glyph. No animation timer, React frame updates, PCM processing, audio meter, or worker was added.
- VoxDisplay observes Codex live voice's existing WebRTC control events (`output_transcript.added`, `turn.created`, `turn.done`) for coarse reply activity. It retains identities and timestamps, not the event's speech text. An old assistant completion cannot stop a newer reply. Backing-agent completion cannot override ongoing voice output.
- Conversation identity, four-turn/3,000-character initial context, transcript-tail flush, native single-writer admission, and Mono-style reserved Vox memory storage remain unchanged. Final displayed reply retention remains capped at 4,000 UTF-16 characters.

## current verification

- All 133 focused Vox, Settings, and app-lifecycle tests passed; TypeScript passed. At the MR checkpoint, the full web suite passed 5,313 tests with 13 skipped; native formatting, Clippy, and 662 native tests passed with one ignored. The local broken Rustup Clippy shim was bypassed with the installed toolchain, without repository changes.
- All 24 Chromium/WebKit checks passed; after the final scroll fix, both focused silent-reply checks passed again with a 30-line reply and real wheel scrolling reaching the final sentence. They cover existing routing/composer/history/memory contracts, persisted silent mode, one-connection silent-mode changes, no extra mic controls, reply bubble placement and dismissal, latest-reply replacement, glyph changes, no animations while stopped, and Reduce Motion.
- Deterministic animation checks paused CSS timelines and sampled glyph visibility: one visible frame per sample, three distinct idle glyphs and at least three talking glyphs. Removing the title-bar control removes its animation effects while preserving the call.
- A real authenticated Codex/WebRTC probe with isolated temporary storage and synthetic speech observed zero startup audio energy, no startup app actions, a completed reply, and the display moving from replying back to listening. The working native channel uses Codex's live turn events rather than the standard Realtime output-buffer event names. Temporary credential copies were removed after the probe. This validates protocol activity, not physical microphone performance or word-level lip sync.
- Native CUA in the isolated development app confirmed same-window title-bar start/end, static stopped Vox, and the Silent replies switch changing on. It also confirmed the control remains available in Settings. Only the isolated Dev app was quit; no production app restart was performed.
- The active design hook reported no deterministic findings for the new Vox CSS. Its edit ceiling suppressed further hints for VoxButton; no extra detector was run. Visual review still judges the rendered result.

## current visual evidence

These 1000 × 800 browser fixture captures use production components with fake
native audio/storage. The named six were opened together; fixtures' navigation
buttons are test controls, not app product copy.

- `.impeccable/review/vox-titlebar-silent-{chromium,webkit}.png`: face control with a compact reply bubble directly below it.
- `.impeccable/review/vox-titlebar-settings-{chromium,webkit}.png`: persisted Silent replies within the shared Audio group, after the first General opt-in.
- `.impeccable/review/vox-titlebar-off-chromium.png`: static `vox` control, no reply bubble or animated frames.
- `.impeccable/review/vox-titlebar-talking-webkit.png`: the compact face during native reply activity.
- `.impeccable/review/vox-titlebar-long-{chromium,webkit}.png`: bounded 180px bubble scrolled to a readable final sentence. Both were opened.
- `.impeccable/review/vox-titlebar-native-{off,settings}.png`: native app title-bar placement and standard Settings; both were opened. The Settings capture includes the existing resource-pressure toast and computer-use pointer annotation.

The fresh finish review found one material gap: the height-capped bubble clipped long replies. Adding scroll, keyboard focus, and a visible focus ring resolved it. The reviewer scored the fix resolved and returned ship, based on both long-reply captures and wheel-scroll tests.

The earlier compact-panel and memory-settings captures remain historical
references. Pre-existing absent PRODUCT.md/DESIGN.md artifacts remain outside
this narrow extension; no project-wide design system repair is included.

## local build and limits

Local app-only Apple Silicon build passed and was installed at `/Users/vrana/Applications/vorkspace.app`. Strict signature verification passed before and after installation; the installed executable matches the build, SHA-256 `cb790b4760a916398bf31c1067e7a80d25dd864ac6fb29c1c1a03a4311c44b4d`. The prior bundle is retained at `/var/folders/0w/0ycdskvx3y51rxg3x9d4n7lh0000gn/T/vorkspace-before-vox-titlebar-hzx7fh_o/vorkspace.app`. The known non-fatal rust-objcopy/libLLVM warning did not prevent compilation, bundling, or verification. The running production app was left untouched; relaunch loads the installed changes. The app install is local only; no release or DMG was published. The reviewed source checkpoint is prepared on `codex/vox` for the MR.
These checks establish functional contracts and animation admission. They do
not establish a measured latency, memory, or battery improvement. A crash or
force kill can still lose native speech that has not flushed. Native call and
conversation persistence behavior remains covered by the earlier isolated
conversation-resume probe (including the “cobalt lantern” recall and reset).
