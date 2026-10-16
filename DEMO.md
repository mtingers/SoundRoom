# Demo projects

Only these two songs are included in this repository:

| Demo | Project | Listening preview |
| --- | --- | --- |
| Twinkle Star · darkwave | `demos/Twinkle-Star/Twinkle.soundroom` | [Twinkle.m4a](demos/Twinkle-Star/Twinkle.m4a) |
| Nutshell · instrumental, steel-string version | `demos/Nutshell/Nutshell.soundroom` | [Nutshell.m4a](demos/Nutshell/Nutshell.m4a) |

Build and launch Soundroom using [BUILDING.md](BUILDING.md), then use **File → Open Session** to open a project. Each `.soundroom` directory is a complete project package: keep `session.json` and its `Audio` directory together. Playback uses the embedded audio and does not require Python, models, or downloaded sample libraries.

The projects preserve instrument notes and drum patterns for editing. Regenerating a recorded-instrument part uses the sample libraries installed on your audio server; without the matching library, its regenerated sound can differ from the supplied clip. The Nutshell version uses a separate FreePats steel-string guitar variant. Save As before experimenting so the supplied project stays available.

The repository includes editable projects and compact AAC previews. Full-mix WAV duplicates, stems, render logs, older arrangements, and the other songs are excluded. A full-quality mix can be exported from each project in Soundroom.

## Credits and licensing

The MIT license applies to Soundroom's own code. It does not assign a new license to an underlying third-party composition, sample library, or demo performance.

- [Twinkle Star notes and credits](demos/Twinkle-Star/README.md): a darkwave arrangement of the traditional public-domain melody.
- [Nutshell notes and credits](demos/Nutshell/README.md): the instrumental demo associated with Alice in Chains' Nutshell; no original recording or lyrics are included.
- [Third-party instrument/drum notices](THIRD_PARTY_NOTICES.md) describe the recorded libraries used by Soundroom.
