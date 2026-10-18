# vox — settings alignment and physical microphone choices

final result: passed

## scope and source truth

This is a scoped UI review, not certification of physical microphone reliability
or a whole-app performance budget. The user superseded the original panel brief:
keep vox grayscale, remove the display of their speech, use the MacBook mic by
default, and provide a dedicated Vox settings section with an input selector. The latest
correction limits lowercase styling to the Vox panel: Settings follows the
incumbent capitalization and layout, and the menu lists physical inputs once
without built-in/system aliases.

The original composition remains the source target:
`/Users/vrana/.codex/visualizations/2026/10/18/01a14c7f-2c0d-74f1-a260-7e71bdc4f73e/vox-panel-preview.html`.
Its capture is `.impeccable/review/vox-approved-preview.jpg` (799 × 1033 pixels).
The latest explicit instructions override its lavender/blue and user captions.
Existing `SettingsView` groups, rows, and selectors own the settings design.

## visual evidence

- `.impeccable/review/vox-final-replies-chromium.png`: 1000 × 800 pixels; grayscale face, assistant reply only, silent replies enabled.
- `.impeccable/review/vox-settings-corrected-chromium.png`: 1000 × 800 pixels; standard Settings capitalization and exactly two physical choices.
- `.impeccable/review/vox-settings-corrected-webkit.png`: 1000 × 800 pixels; same state in WebKit.
- `.impeccable/review/vox-settings-corrected-iab.jpg`: 1280 × 720 pixels; corrected open selector at the user browser size.
- `.impeccable/review/settings-general-baseline-iab.jpg`: 1280 × 720 pixels; incumbent General settings as visual authority for this correction.
- User-supplied `codex-clipboard-915ab78c-a674-4d23-93d4-0de13c4cf6f0.png`: the rejected lowercase/four-option state, primary evidence superseding the earlier review.

Each capture was opened and validated. Reference and rendered implementation were
viewed together in a single comparison input. These are 1:1 CSS-pixel captures;
surrounding preview/fixture canvases differ, so panel comparison uses their native
340px square regions rather than full-canvas pixel similarity. Panel face, footer,
reply text, and the settings row/options are readable at capture size and were
inspected directly. Device names and caption content in browser fixtures are
synthetic. No raster artwork or image substitutions are involved.

## findings

No actionable P0/P1/P2 visual differences remain. A fresh Impeccable review of the user's correction returned
`disposition: ship`; no material fixes. It confirmed that Vox and General share
the same 160px content inset, heading position, content width, and row padding at
1280 × 720 pixels. Documentation handoff confirmed that this
extends the incumbent UI system without creating a new design system.

| required surface     | result                                                                                                                                                                                                                                                                                                         |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| fonts and typography | Existing body typography, compact lowercase controls/replies, and the approved 76px mono letter/symbol face. Settings hierarchy follows incumbent headings and rows.                                                                                                                                           |
| spacing and layout   | Existing anchored 340px square panel and 16px inset; bounded reply area, stable footer. Settings uses existing Group/Row/Select composition with no clipped options at reviewed widths.                                                                                                                        |
| colors and tokens    | Face/focus now use grayscale content tokens. Shared glass, selection, borders, and theme behavior remain. No new global palette or surface material.                                                                                                                                                           |
| image/asset quality  | User-requested text faces and existing Hugeicons. No generated art, custom glyph icons, new fonts, or image approximations.                                                                                                                                                                                    |
| copy and content     | Only the Vox panel uses lowercase; Settings follows the app's capitalization and native device labels. User speech is absent from the panel; raw reply and app-action payload case is preserved underneath presentation. Selector states that the next call uses the preference and system input is unchanged. |

## functional evidence and ownership

- Partial transcript notifications are suppressed in Codex before stdout/IPC. The display stores only native completed assistant replies. User final text is discarded; it can reset a model mood without creating a caption. There are no caption batching timers or per-word panel updates.
- Retention remains bounded to 12 replies, 4,000 UTF-16 characters per reply and 16,000 total, with truncation. Nothing is persisted as chat history.
- Silent replies and mic-off independently control the existing speaker element and input track. Closing/reopening the panel preserves the call and replies; end releases it.
- Settings → Vox is indexed in navigation/search. Its menu lists only actual named inputs; built-in/system aliases are absent. The built-in default resolves to its physical device using the same resolver as capture. Explicit device selections persist for the next call. A saved missing device remains readable in the trigger without adding another choice. Previously saved system-default preferences remain honored until changed. Settings enumeration never captures audio on mount/open and listens for device changes only while mounted.
- Built-in selection resolves the internal microphone and requests its exact device ID. Missing or removed inputs fail with settings recovery rather than falling back to AirPods. Hidden device labels can require one permission-only capture, stopped before any peer/helper starts. Cancellation prevents later admission and releases a late permission grant.
- Routing instructions now require active-chat/history/status requests to look up the app state through the backing agent rather than claiming that the selected chat is invisible.
- Fourteen Chromium/WebKit checks passed, including settings reload/next-call capture constraints, no user captions, call continuity, and independent mic/speaker controls. Browser page errors were collected in the new settings scenario and were empty.
- Before this correction, the full web suite passed 5,290 tests with 13 existing live-provider skips. This correction passed 109 targeted settings/microphone tests and all 14 Chromium/WebKit checks, including alias filtering, native capitalization, persistence, and next-call exact-device capture. TypeScript and the app-only Apple Silicon build passed.
- An isolated native macOS WKWebView plus the real harness and Codex returned a complete synthetic greeting reply and a second spoken-turn response. A separate live browser probe waited for a completed backing turn and its final spoken/text answer using synthetic chat history, rather than accepting an acknowledgement filler as proof of completion.
- The final production-prompt probe completed `workspace.current` and `sessions.read`, then returned the synthetic session's last reply after the backing turn finished. Silent startup produced no reply or backing turn, and no user text entered the displayed reply list.
- The corrected signed bundle was installed at `/Users/vrana/Applications/vorkspace.app` and passed strict signature verification. Its executable matches the built bundle (SHA-256 `dcfa1e16e2ac635276da184a24be5467d9d9f13b7331e0da607dc82edf3494c0`). The previous bundle is retained for rollback; the running app was left untouched. No updater artifacts or DMG were built.

## comparison and investigation history

The first panel review passed its original visual brief. User feedback superseded
that brief and reported stalled input. The current review replaces the earlier
blue/two-sided-caption and lowercase/four-option settings evidence: color and user speech display were removed,
partial notifications were suppressed, and a dedicated input setting was added.
The system input was observed to be AirPods; the user explicitly selected the
MacBook microphone for vox. Bluetooth route trouble is a hypothesis, not a proven
root cause. Real synthetic calls established a working response path but did not
reproduce the physical-input failure.

One browser run was interrupted by a Vite reload while temporary native-probe
files were removed. The frozen-file rerun passed all fourteen cases. An omitted
settings-rail icon was caught by TypeScript and added using the existing mic icon.

## remaining scope

The user's original AirPods stall has not been established by a physical
AirPods/MacBook A/B test. Repeated synthetic calls and unit/browser checks are not
a latency or memory benchmark. Remote model routing and silent startup remain
provider behavior. The local installed app must be relaunched to load the changes.
No MR, push, release, or DMG is part of this work.
