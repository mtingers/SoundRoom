# Portable Soundroom TUI

A server control client built on `soundroom-client`. It never opens an audio device
or schedules local audio. The native Mac TUI remains available while this portable
client is qualified and its advanced workspaces are completed.

## Build and connect

```sh
CARGO_HOME=/private/tmp/soundroom-cargo cargo build --offline \
  --manifest-path portable/Cargo.toml -p soundroom-tui --release
portable/target/release/soundroom-tui \
  --server wss://studio.local:49153/control --credential approved-client.json \
  --name 'Approved client name' --ca studio-ca.pem --origin https://studio.local:49152
```

The approved name must exactly match the host's credential grant. Credentials are
loaded from an owner-only, nonsymlink file; the SDK verifies TLS hostname and
certificate trust, authenticates a challenge and verifies complete snapshots.
The optional private CA file adds trust without disabling normal verification.
No credential or pairing code goes in a URL. Windows console support is present,
but actual Windows/Linux credential, trust and terminal qualification remain
release gates. The shared SDK supplies platform-specific private credential storage.

## Controls

| Action | Keys or command |
|---|---|
| Private track/cursor navigation | `j/k`, `h/l`, arrows, `gg/G`, `w/b` clip edges, Page Up/Down; counts such as `3j` or `8l` |
| Private scroll/zoom | `H/L`, `+/-`; manual scrolling suspends Follow |
| Time/Beats, Snap, Follow | `B`, `n`, `:follow` |
| Choose clip / select a range | `w/b`; `v`, then `h/l`; `:select` selects intersecting clips; `[` / `]` nudge a clip |
| Mouse | Click track/clip to select and place cursor; wheel moves tracks |
| Server play/pause / selected track | Space / `:play-track` (operator only) |
| Record / finish / take transport | `r/R` / `:finish` / `P` (operator only) |
| Mute, solo, arm | `m`, `s`, `a` |
| Split, delete, combine | `x/S`, `dd` or Delete, `C` or `:combine` |
| Private clip clipboard | `y` copies source references or the selected audio range; `p` pastes with source and target revision guards; `:cut` cuts a range without closing the gap |
| Track clipboard | `:copy-track [automation]`, then `p`; independent effects/takes, preserved baked audio guards and exact relocated automation |
| Clip inspector and history | Right-click a clip or `:clip` / `:history`; `:restore 1` restores the latest previous state; `:history-track 1` creates a track at the current clip position |
| Source boundaries and placement | `:trim start end`, `:trim-range`, `:slip sourceOffset`, `:align`; exact frames or `seconds:2.5` / `beats:4.25` |
| Fades and join protection | `:fade in ms:5`, `:fade out seconds:0.5`, `:protect on`, `:crossfade ms:20` for touching/overlapping clips |
| Author-scoped Undo/Redo | `u`, Ctrl-R |
| Mixer values | `:gain -6` (dB), `:pan -0.2` |
| Add/rename track | `:add "Track name"`, `:rename "New name"` |
| Host input routing | `i`, then `:route HOST_UID 0 [stereo]` (zero-based channel) |
| Effects | `e`; `E/B` insert, `J/K` parameter, `h/l` value |
| Effect editing | `:fx-add ID`, `:fx-set parameterID value`, `:fx-preset index`, `:fx-toggle`, `:fx-remove` |
| Bake live insert chain | `:apply`; server Undo and clip history retain the source |
| Loop reviewed private range | `:loop`, `:unloop` |
| Local audio import | `:import "local recording.wav" [new]`; current track by default, cursor placement; host codec support applies |
| Mix export / verified download | `:export "local mix.wav" ["Mix name"]`; `:files`, then `:download 1 "another destination.wav"` retries downloading a recent manifest |
| Transfer progress / cancellation | `:transfer`, `:transfer-cancel`; Stop/playback remain available while hashing and transferring |
| Host projects | `:projects`, `:open ID`, `:save "Name"` |
| Connection | `:refresh`, `:reconnect` (explicit; sent edits are never replayed) |
| Help / exit | `?`, `q` or Ctrl-C |

`Tab` switches arrangement/effects/routing. Escape clears range and clip selection.
The command line consumes typing and bracketed paste without dispatching global
shortcuts. Quotes and escapes only group arguments; commands never invoke a shell.

Navigation is client-private. Shared edits carry the displayed global and target
track revisions, and a second edit waits for the prior receipt and corresponding
metadata. A conflict or missing receipt is visible; an uncertain edit is never
retried automatically. Read-only viewers can navigate, refresh and list projects,
but cannot edit or operate transport. Unsupported host capabilities are rejected
before sending a control command.

## Designer and Beats

`:designer` and `:beats` open private drafts from a selected generated clip or
an advertised host preset. Space starts/stops that workspace's host audition;
it does not trigger the background arrangement. `Tab` cycles the note/hit grid,
sound controls and labelled keyboard. Escape or `T` returns to the timeline and releases
this client's audition. An operator credential is required to audition.

Designer grid: `n` adds a note, `h/l` moves its time, `j/k` its pitch, `[`/`]` its
length, `J/K` selects notes and Delete removes one. `+/-` zooms. Sound focus uses
`j/k` to choose a physical parameter and `h/l` to adjust it. Keyboard focus labels
`awsedftgyhujk` with their pitches; `z/x` changes octave and `r` records taps into
the private draft. Terminal taps release after 300 ms on the host.

Beats grid: `h/l` selects a step, `j/k` a drum, Enter toggles a hit; mouse clicks
use the visible scrolled grid. Sound focus edits tempo, bars, division, swing,
timing/velocity variation, master gain and variation cycles. Keyboard focus
labels `asdfghjkl` with drum voices.

| Creative command | Purpose |
|---|---|
| `:presets`, `:instruments`, `:kits` | Browse actual advertised banks; `j/k` scroll, Tab returns |
| `:preset index`, `:instrument ID`, `:kit ID` | Load a bank sound or drum kit |
| `:sound parameter value`, `:seed unsigned64` | Precise synthesis/organic controls and deterministic seed |
| `:waveform waveform\|secondWaveform sine\|triangle\|saw\|pulse` | Oscillator waveforms |
| `:layers`, `:layer-add index`, `:layer-remove index` | Browse/edit up to three additional native layers |
| `:layer-set index parameter value` | Gain/pan/transpose, velocity range/fade, envelope/tone/character overrides and seed |
| `:controllers`, `:controller kind beat value`, `:controller-clear kind` | Expression, pitch bend, modulation and sustain points |
| `:velocity value`, `:hit parameter value` | Note velocity or hit velocity/pan/probability/articulation |
| `:lane parameter value`, `:beat parameter value` | Drum kit/gain/pan/tuning/damping/mute and pattern controls |
| `:insert`, `:audition`, `:help` | Render to the host timeline, audition or list exact syntax |

`u`/Ctrl-R undo/redo the private draft. Editing during audition coalesces to the
latest pattern on the 50 ms UI tick; the host renders away from its audio callback.
Existing generated clips keep their opening track revision when inserted, so a
concurrent edit causes a conflict instead of overwriting another client's work.
Factory Designer synthesis changes preserve the current sequence and controllers.
Grid changes preserve drum timing and the stronger colliding hit; shrinking bars
removes hidden hits. Native physical ranges, velocity windows and host-advertised
articulations are validated before commands are queued.

## Automation

`:automation volume`, `:automation pan`, `:automation amount`, or
`:automation fx parameterID` opens the chosen track lane. The curve chart
supports mouse point creation, `n` to add, `J/K` to select points, `h/l` to move
time, `j/k` to change values, Delete, `+/-` zoom and `H/L` scroll. `u`/Ctrl-R undo
private point and interpolation-scale edits. Space controls the server mix.

`:point beats:4.25 value` or `:point seconds:2.5 value` aligns an exact point;
an integer audio frame is also accepted. Volume values are dB, pan is -1..1,
and effect values use their labelled physical units (percent controls use 0..100).
`:value value`, `:curve linear|smooth|easeIn|easeOut|hold`, `:smooth`, `:clear` and
volume `:scale decibels|linear` provide precise editing. Enter or `:commit` applies
the draft and closes the chart. Escape discards the private draft.

The server receives an unchanged rack/routing plus the changed curve under the
opening track revision. Concurrent edits produce a conflict. Curve rendering
honors clipped easing domains and the native volume interpolation scale; legacy
amount-only inserts are upgraded with their original sound and automation before
physical parameter automation is edited. Continuous fader Write capture remains
pending; this workspace provides precise authored curve editing and playback.

## Pitch correction

Select a clip or range and a Pitch correction insert, then `:pitch`. Analysis and
rendering run on the hardware server. `:analyze` measures the original;
`:measure` measures processed audio. Space auditions the processed range, and
`:original` auditions its source. Preview requires an operator credential.
`:preset Natural|Tight|Robotic|Keep performance` applies native target presets.

The chart keeps a fixed vertical view during playback. `+/-` zooms time, `H/L`
scrolls, `z/Z` zooms pitch and PageUp/Down shifts the pitch view. Confident source
points are cyan, processed points green and editable targets blue. `n` adds a
target; `J/K` selects points, `h/l` moves time, `j/k` changes pitch, and Delete
removes a point. Mouse creates a target at the chart coordinate. `:point frame C#4`
or a fractional MIDI note makes a precise edit; `:smooth` and `:curve` choose
native easing. `:parameter ID value` uses advertised physical units, and
`:processing-preset index` changes synthesis settings while retaining manual
curves. `u`/Ctrl-R restores private target and processing changes together.

Enter or `:use` installs the draft under its opening track revision and closes;
`:apply` rewrites audio using the already installed live rack. Private targets
must be used before applying or processed audition. Original audition remains
available while reviewing private edits. Closing cancels only this client's
Pitch preparation/audition; it never stops the main mix or cancels unrelated
shared work. Responses carry a private workspace identity and source revision,
so a closed workspace or concurrently changed track cannot adopt stale analysis.
Target curves outside the reviewed range retain their clipped nonlinear easing.

## Master the mix

`:master` opens measurements for the server's Master track. `:analyze` measures
the full mix on the host; after the authoritative metadata arrives, `:proposal`
requests its native processing rack. `:target -14` (−24..−8 LUFS) and
`:intensity 0.65` set the next proposal's loudness reference and correction
amount. Loudness aims are references, not guaranteed output measurements.
The chart shows measured section loudness and true peak, with integrated LUFS,
crest and bass/air energy readouts. `j/k` selects measured sections, `+/-` zooms,
`H/L` scrolls and mouse selects the visible section. Chart samples are cached
until measurements or the private viewport change.

Tab focuses the proposed rack. `J/K` selects an insert, `j/k` its parameter,
`h/l` changes a physical value and `m` bypasses it. `:effect index`,
`:parameter ID value`, `:processing-preset index` and `:rack` provide exact
control and a chain summary. `:automation parameterID` opens a private curve
editor for the proposed insert; Enter returns the edited curve to the proposal,
and Escape discards that curve draft. `u`/Ctrl-R undo/redo private rack and curve
changes together. Enter or `:use` applies the complete proposal under the
opening Master revision and closes only after the accepted receipt. Conflicts
and local queue rejections retain the private proposal. The host validates the original mix
fingerprint and retains its previous rack in authored Undo.

Space plays/pauses the shared mix with an operator credential. Viewers can
request and inspect a proposal from existing measurements; editors/operators
can analyze and use it. `:cancel` explicitly requests cancellation of the
client's job (the host checks ownership/operator authority). Closing the view
ignores late results without cancelling shared work. Mix edits invalidate the
proposal; replies from older query identities are ignored. Concurrent Master
edits retain the opening target revision and produce a conflict on apply.

## Performance and safety

A dedicated control worker owns the authenticated socket. The UI draws at most
20 times per second with ANSI cell diffs, visible track rows and indexed visible
clips. Immutable metadata is shared by `Arc`; transport/meter events do not copy
it each UI frame. Designer notes use a pitch/time index rebuilt only after edits;
large note sets are not rescanned on playhead frames. Pitch targets cache their
curves and time index until edits; redraw only visits visible points. Private creative undo is
bounded to 100 states and 32 MiB. The timeline draws the host's waveform envelope with clip source
offsets. Beat snapping calculates absolute boundaries at the project tempo; it
does not accumulate sample rounding. Playhead interpolation expires after 500 ms.

Worker queues are bounded to 56 ordinary and 8 reserved priority requests with a
32 MiB queue budget with 1 MiB reserved for interrupts; each control fits the
8 MiB host frame limit. The SDK independently limits total pending bytes.
Pause, stop, note-off and cancellation use the reserved lane. Terminal input,
mouse coordinates, paste, cells, and output dimensions are bounded. Untrusted
names cannot inject terminal escape sequences. Unix raw mode and Windows console
modes, alternate screen, mouse and paste reporting are restored by an RAII guard
on exit/error/panic; signal/console exit requests are handled on the main loop.
SIGKILL cannot run cleanup. Rendering currently uses an ASCII cell alphabet.

## Qualification and remaining parity

Pure tests cover fragmented UTF-8/mouse/paste, private view identity across server
reorder, author revision guards and receipt-before-metadata barriers, role gates,
host routing, focus-safe input, differential rendering and a 100,000-clip query.
Tests do not establish actual terminal interaction, LAN WSS trust, recording or
Windows/Linux acceptance. Desktop/terminal application checks are queued without real terminal and hardware qualification.

Designer/Beats native preset file interchange and MIDI capture/import/export, live fader Write capture,
shared presets, device output selectors, keyboard
accessibility and Unicode width support still require dedicated work. These
workspaces are functional server controls; full native UI parity remains pending.

Local queue failures are separate from server receipts and release only the matching waiting editor. A sent edit with an uncertain outcome stays blocked until an explicit fresh server snapshot; unrelated metadata changes do not count as receipt recovery. Reconnect never replays edits, and late receipts from an abandoned connection cannot satisfy a newer edit barrier.

Track copying without automation clears manual pitch targets and their manual-only gate, while retaining baked ranges to avoid applying an already rendered effect twice. Copying with automation preserves curve domains at clipped boundaries (including dB volume) and remaps nested effect, region, point, take and clip identities. Source changes require copying again rather than silently pasting a different revision. Clip history is ordered newest first and restored at the clip's current timeline position; the host remains authoritative for history capture and validation.

Local file transfer uses the same authenticated peer as editing and Undo. A separate bounded disk worker hashes or writes files, while the control worker sends one receipt-driven chunk at a time (up to 256 KiB; 2 GiB maximum). Local paths never cross the wire. Upload targets keep the opening track/session revision, so a changed target rejects safely. Final import waits for other authored receipts and metadata before being sent. Downloads verify SHA-256 and exact chunk offsets/completion before atomic no-clobber installation; failed or canceled downloads discard partial files. Recent export manifests are client-private, capped at 16, and may expire on the host. Exports currently use the host's WAV mix command; other export formats need a separate workflow.

Canceling a sent import or export cannot retract the host job. Its exact request identity is retained and displayed; the client never replays it automatically. Refresh authoritative metadata before retrying an uncertain import. Queued-only cancellation uses the SDK's cancel_queued API, preserving sent and uncertain bytes for explicit reconciliation.

### Local MIDI and custom presets

Inside Designer, use `:midi-load "song.mid"`, then `:midi-parts` to inspect tracks/channels, note counts, tempo and duration. Loading lists parts without replacing the current sequence. Choose an explicit excerpt with `:midi-excerpt 0 16 8` (part 0, start beat 16, eight beats). Crossing notes are clipped and controller state is carried into the excerpt. The current instrument, layers and organic settings stay intact. Undo restores the sequence; `:insert` sends the authored render to the host.

SMF type 0/1 and PPQ timing are supported, with a 4 MiB file limit, 200,000 events and 65,536 beats. Excerpts support up to 64 beats, 256 notes and 4096 controller points; dense excerpts reject with a request for a shorter range instead of discarding notes. Variable-tempo, SMPTE and independent type 2 files need conversion first. `:midi-save "new.mid"` exports notes and expression, modulation, sustain and pitch-bend controllers at 960 PPQ. Instrument synthesis stays Soundroom-native; the file does not promise General MIDI instrument mappings.

`:preset-save "new.soundroom-sound.json"` saves a native `DesignerSoundPreset` version 1 with exact UInt64 organic seeds. `:preset-load "my.soundroom-sound.json"` replaces sound controls while preserving notes, controllers and tempo. In Beats, the same commands exchange native `DrumPattern` JSON including hits, lanes, kit variations and articulations. Presets are limited to 512 KiB and validate supported ranges. A preset cannot overwrite edits made while it was loading, and closed-workspace responses are ignored.

Parsing, hashing and local file access run off the terminal/control polling threads with bounded queues. File saves use a temporary file and atomic no-clobber installation; choose a fresh destination filename. Client-local paths never enter server requests. These workflows are available in private drafts to viewers; server render/preview operations retain their normal role restrictions. Live hardware MIDI recording remains a separate gate: incoming note events do not yet contain a host sample clock/epoch, so clients do not invent capture timing from network arrival.

### Export recipes and hardware output

`:export "local mix.aiff" "Mix name" selection no-tails` requests a host render in the format inferred from the destination extension. WAV and AIFF are offered when advertised; other codecs must also be explicitly advertised by the host. `selection` uses this client's timeline range; `no-tails` excludes effect decay after its end. Omit both for the complete mix with tails. Plain WAV export remains compatible with hosts advertising only `exportMix`; extended recipes require `project.export`.

`:stems wav all` renders one host artifact per track, up to sixteen tracks per request. Use `track` instead of `all` for the current track, and optionally add `selection`, `no-tails` and a quoted export name. Every returned manifest is validated before the set is installed. `:files` lists the stems; `:download 1 "local stem.wav"` saves one using the streaming digest/no-clobber workflow. Automatic folder assembly is still pending. Renderer timeouts retain the exact request identity without automatically repeating an export.

`:projects` displays a numbered host project list. `:open 2` opens its second item; exact host identifiers are also accepted. `:devices` opens host routing. `:output HOST_UID` changes the host output while preserving its current input; this requires an operator credential and stopped host playback/recording. Track input assignments continue to use `:route HOST_UID channel [stereo]`.

### Personal effect presets

`:fx-preset-save "new preset.json" "Preset name"` saves the selected insert's controls in the native macOS `EffectPresetDocument` version 1 format, and adds it to the private TUI library only after the atomic file save succeeds. Legacy processors retain their original `legacyAmount`; automation and baked ranges stay in the project.

`:fx-presets-load "presets.json"` imports a native library privately. It validates effect kinds and parameter domains against the connected host, supports 1–128 entries / 1 MiB, and rejects malformed or duplicate entries before installation. Loading does not modify any track. Duplicate names from an already loaded library gain a suffix. `:fx-custom-list` opens a separate preset browser: j/k, Page Up/Down, g/G and mouse wheel browse; Enter explicitly applies the selected preset to the current insert. t/Escape returns to inserts. The background timeline does not receive preset navigation or delete keys.

`:fx-custom 0` applies a named custom preset with current session/track revision guards and normal server Undo. Existing effect automation, manual pitch targets and baked-audio guards are retained. `:fx-custom-remove 0` removes an entry from the private library; original files are kept. `:fx-presets-save "new library.json"` exports the loaded personal library for use in another TUI session or the macOS app. File destinations must be unused. Viewers can import/export their personal library, but cannot apply presets to authored tracks. Factory presets remain available through `:fx-preset index`.
