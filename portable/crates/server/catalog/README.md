# Server capability catalog

`native-v3.json` contains public effect parameter units/ranges/defaults, factory presets, Designer instruments, and drum metadata. It contains no saved user session, audio-device inventory, or pairing credentials.

The service substitutes its actual supported commands, versions, codecs, backends, and features rather than advertising every native-host workflow. A unit test validates the resulting capability model against the generated protocol grammar.

The native exporter is `RemoteSnapshotPagesTests`, explicitly enabled with `SOUNDROOM_EXPORT_CAPABILITIES`. Keep parameter definitions synchronized with `SoundroomCore`. Sound Putty's 900 sculpt samples use the numeric parameter map and are deliberately absent from the scalar automation catalog.
