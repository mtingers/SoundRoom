# Soundroom terminal client

`SoundroomTUI` is a native terminal control client. It opens no audio or MIDI devices: every playback, recording, edit, effect, route and export runs on the Soundroom server's Mac.

Launch `Soundroom.app` to start the hardware server, then run:

```sh
build/Soundroom.app/Contents/MacOS/SoundroomTUI
build/Soundroom.app/Contents/MacOS/SoundroomTUI --host studio-mac.local --credential /path/to/paired-operator.json
```

The default credential is `~/Library/Application Support/Soundroom/Server/operator.json`. Credentials must be private regular files owned by the current user (mode 0600); locate the credential through the server connection panel and transfer a remote credential securely. The client authenticates the server using its pinned identity. The default port is 49152. Never put the credential contents in command arguments.

Development source can also use the common v3 endpoint: `SoundroomTUI --wss wss://studio-mac.local:49155/ --origin https://studio-mac.local:49154 --credential /path/to/paired.json`. Configure the host and system CA trust as described in [WEB_ACCESS.md](WEB_ACCESS.md). Choose WSS/origin or host/port; they cannot be mixed. This path accepts existing native pairing credentials or v3 portable-server credentials, performs system certificate/hostname verification, and opens no client audio devices. Portable credentials require `--client-name "Approved name"` with the exact operator-approved name; use `/control` for the portable service endpoint. A v3 credential cannot authenticate the legacy host/port transport. Its live acceptance is queued and it is not included in preserved build 34.

## Independent views and shared audio

`j`/`k` selects tracks. `h`/`l` moves your edit cursor by 250 ms; counts work (`10l`, `2j`). `w`/`b` visits clip boundaries, `gg` goes to the session start and `G` to its end. `+`/`-` zooms your view; `:zoom 10` shows ten seconds. These motions affect only your client.

**Enter** explicitly seeks the shared server to your cursor. **Space** plays/pauses shared transport; `r` records/stops using the server's armed tracks and routes. Any transport ownership rejection is shown as an error; `:takeover` explicitly requests ownership. The transport row shows server position separately from your edit cursor.

`v` selects a local time range on the selected track; `V` selects the clip under your cursor. `y` copies selected clip references to your private clipboard; `p` creates new clip identities at your cursor on the selected destination track. Source clips must still exist at paste time. `dd` removes selected clips, never a whole track. `:split` splits a single clip at your cursor. `u` and Ctrl-R request authored Undo and Redo. Escape clears local selection or leaves command mode.

`:remove-track confirm` explicitly removes the selected track. Other editors' newer conflicting edits are rejected through per-track revisions; the client never silently retries a rejected mutation.

## Commands

Type `:` to enter command mode; Tab completes command names, or shows candidates. `?` opens a help overlay. Track commands target your local `j`/`k` selection.

| Command | Action |
| --- | --- |
| `:inputs`, `:outputs` | List devices on the server Mac with IDs/UIDs and channel counts |
| `:input ID-or-UID [channel] [stereo]` | Assign the selected track's input; human channel numbers start at 1 |
| `:input default` | Follow the server's default input |
| `:output ID-or-UID` | Select the server's hardware output |
| `:arm on`, `:mute off`, `:solo on` | Set the selected track state; omit argument to toggle |
| `:volume 0.8`, `:pan -0.2` | Set linear gain (0–4) or pan (−1–1) |
| `:rename Voice`, `:new-track Bass` | Rename the selected track or add one |
| `:effect` | List insert numbers/kinds and bypass states |
| `:effect add echo` | Add a native insert with its physical defaults |
| `:effect bypass 1 on`, `:effect remove 1` | Bypass or remove the numbered insert |
| `:effect set 1 mix 0.25` | Set a native physical parameter; invalid keys list available parameters |
| `:projects`, `:load ID`, `:save NAME` | List, open or save projects in server storage |
| `:export NAME` | Export the shared mix into server storage |
| `:seek 12.5` | Seek shared transport explicitly to seconds |
| `:stop`, `:record`, `:undo`, `:redo` | Shared transport or authored edit commands |

Project/export arguments name managed server storage, not paths on the client. A rejected, busy or conflicting server acknowledgement stays visible in the status row.

## Scripts

```sh
build/Soundroom.app/Contents/MacOS/SoundroomTUI --status
build/Soundroom.app/Contents/MacOS/SoundroomTUI --track Voice --send 'mute on'
build/Soundroom.app/Contents/MacOS/SoundroomTUI --track 2 --send 'arm on'
build/Soundroom.app/Contents/MacOS/SoundroomTUI --send 'play'
build/Soundroom.app/Contents/MacOS/SoundroomTUI --send 'stop'
```

Build 34 bundles the executable beside the GUI. Development builds also provide `.build/release/SoundroomTUI`.

`--status` emits the authoritative JSON snapshot. `--send` emits the server acknowledgement and exits nonzero on rejection; `--track` accepts a name, one-based number, or UUID. Each process normally has a unique client identity; add `--client-id UUID` to sequential CLI calls when you want authored Undo to span those calls. Simultaneous clients should use different IDs. `--import /path/to/take.wav` streams local audio into the server; `--import /path/to/Session.soundroom` packages and streams a project into managed server storage. Add `--track Voice` to choose the audio destination; imported projects open as server sessions.

An export acknowledgement contains an artifact UUID. `--download UUID --output /path/to/new-mix.wav` saves it locally in bounded chunks and refuses to overwrite an existing file. Imports and downloads are capped at 2 GiB. Server export artifacts retain their original bytes even if a later export uses the same human-readable name (the latest 256 artifact versions are retained).

Connection options also work in scripts. `--help` requires no connection or credential.

## Lifecycle and performance

`q`, `:quit`, Ctrl-C, SIGTERM and SIGHUP detach and restore the terminal; detaching does not stop server playback or recording. Connection loss leaves a visible disconnected screen rather than silently resending edits. Restart the client to reconnect with a fresh snapshot.

The TUI draws only when dirty, with a 10 Hz cap, bounded terminal dimensions, no waveform/PCM transfers, and at most 32 queued edits plus a separate eight-entry transport queue. Stop/pause/play/record/seek and takeover can reach the server while an export or save is awaiting completion; the server still decides whether they are allowed. Transport frames update independently of track projection. Per-track clip projections and clip-ID sets survive continuous gain/pan/mute changes, so controls do not reconstruct long timelines. It ignores audio meter events. A slow terminal cannot block server audio. Terminal control characters from names/errors are stripped before display.

## Mouse, color and inspectors

The terminal client now has a color timeline and a mouse interface. `--no-color` or `NO_COLOR=1` disables color; `--no-mouse` leaves mouse selection/scrolling to your terminal. Mouse reporting requires a terminal that supports SGR mouse events. The TUI restores mouse reporting, bracketed paste, colors, the alternate screen and terminal input on normal quit and handled signals. Pasted text is accepted only in command mode and cannot fire audio/edit shortcuts.

- Click a track label to select it. Click its `R`, `M`, or `S` flag to arm, mute or solo that track. Lowercase flags are inactive, also in plain-color mode.
- Click a clip to select it and place your private cursor. Shift-click adds/removes clips on that track. Drag across the timeline to select a time range. **Enter** or **Seek** explicitly moves shared playback.
- Wheel scrolls tracks or inspector content. Shift-wheel pans the timeline; Ctrl-wheel zooms. Terminal applications may intercept modifier-wheel combinations.
- Click the transport or editing toolbar. Split, Combine, Copy, Paste and Undo are directly accessible.
- Click **Mixer**, **Effects**, or **Routing** to switch the inspector. A wide terminal puts it beside the timeline; a narrower terminal puts it below. At very small heights use commands or enlarge the terminal to see inspector controls.
- Mixer offers gain/pan bars, track audition, rename, new track and duplication. Effects shows every native physical parameter and factory preset; click a value for exact command entry, a bar to set its value, or a preset to apply it. Presets retain existing automation. Routing lists the server's inputs/outputs and lets you choose them directly; channel/stereo changes use the inline command prompt.

`Tab` changes keyboard focus between timeline and inspector. `t` returns to the timeline. `e` cycles inspector panels; `f` opens Effects. In Mixer, `j/k` chooses gain/pan and `h/l` adjusts it. In Effects, `J/K` chooses an insert, `j/k` chooses a parameter, `h/l` adjusts it using its native linear/logarithmic/discrete scale, and Enter starts exact value entry. Space remains shared play/pause in both panes. Ctrl-P or Up in command mode recalls the last command.

New direct shortcuts: `m` mute, `s` solo, `a` arm, `x` split, `C` combine selected clips, `L` loop selected clips/range (or disable the current loop), `F` fit the session, and `[` / `]` nudge selected clips by one cursor step.

| Command | Action |
| --- | --- |
| `:play-track` | Audition the selected track through the server |
| `:combine` | Render selected clips into one on the server; gaps are retained |
| `:loop`, `:loop 2 12`, `:loop off` | Loop selection, explicit seconds, or disable |
| `:move 12`, `:nudge -0.25` | Move selected clips to seconds or offset them, preserving spacing |
| `:trim 2 4` | Trim one selected clip to absolute timeline seconds; retain the other clips |
| `:duplicate-track` | Copy the track at its original timeline position without automation |
| `:duplicate-track automation` | Copy the track including mix/effect automation at its original position |
| `:effect presets 1` | List factory presets for insert 1 |
| `:effect preset 1 2` | Apply factory preset 2; full preset names also work |
| `:cancel` | Cancel the current server render/job when authorized |
| `:fit` | Fit the session in your private timeline view |

Track duplication creates new track, clip and insert identities and does not arm the new track. It copies the current clips and insert chain, rather than hidden recording takes. It keeps media references on the server. Destructive edits participate in the server's authored Undo/conflict checks. Combine is a cancellable server render and can take longer on long audio; audio transport uses the separate urgent command queue.

This pass expands arrangement, mixer, effects and routing control. Designer/Beats piano-roll or step editors, manual pitch charts, automation curve drawing and master analysis workspaces remain GUI features. This TUI does not yet offer full GUI workspace parity.

Operators can use `:web-pair viewer|editor|operator CLIENT NAME`, `:web-clients` and `:web-revoke CREDENTIAL_UUID` to manage the opt-in browser endpoint. The Mac connection sheet also has Browser access controls. Pairing codes last two minutes and work once; revocation disconnects only that credential's clients. See [HTTPS/WSS setup](WEB_ACCESS.md).

## Musical timeline

Start with `SoundroomTUI --beats`, click **Time/Beats**, or press **B** to switch your private timeline display. In Beats, ruler and cursor use `bar.beat.tick` (one-based bars/beats, 960 ticks per quarter note). `h/l` moves by the current musical division; counts and visual selection work the same way. The GUI has the same **Time/Beats** selector and shared project tempo/meter controls.

| Command | Action |
| --- | --- |
| `:timebase beats`, `:timebase time` | Choose your private display |
| `:tempo 123` | Change shared project BPM (20–300) |
| `:meter 6/8` | Change shared meter; numerator 1–16, denominator 2/4/8/16 |
| `:grid 1/16`, `:grid triplet`, `:grid bar`, `:grid off` | Choose private beat snap division or disable snapping |
| `:seek 5.1`, `:loop 5.1 9.1` | Seek or loop using musical coordinates in Beats |
| `:move 5.1`, `:nudge 1` | Move to a musical position or nudge one meter beat in Beats |

In Time, seek/loop/move/trim positions and nudge amounts remain seconds. `:zoom` always specifies the visible duration in seconds. Changing tempo or meter changes coordinates and recording count-in/metronome; it does not stretch existing audio or move clips. BPM counts quarter notes even in 6/8. Timing is saved in the project and supports authored Undo; display/grid/scroll choices stay local. Projects with nondefault timing require Soundroom project format 9.

## Portable Rust client in development

The new `soundroom-tui` uses the verified common WSS SDK on Mac, Windows and Linux. Build with `cargo build --locked --release --manifest-path portable/Cargo.toml -p soundroom-tui`, then follow [connection and control instructions](portable/crates/tui/README.md). Supply the private credential file and exact operator-approved name; the client opens no audio devices.

Arranging, mixer, routing, factory effect controls, project commands and authored Undo have offline test coverage. Advanced workspace/clipboard/transfer parity and real terminal/platform acceptance are still underway. The Rust client is experimental; the macOS app bundle includes the native Swift terminal controller. Real terminal/platform checks are separate from offline tests.
