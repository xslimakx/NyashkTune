## NyashkTune v1.0.1

NyashkTune is a realtime vocal pitch correction (autotune) VST3 plugin.

### New in this build
- **Selectable formant algorithm**: Classic, Smooth, Detailed, Vocal (LPC), Bend - plus Tilt and Follow, and a live formant map.
- **2 extra voices** (harmony / octave / doubler) with scale-aware intervals, level, pan, detune, formant and delay.
- **Antares-style extras**: Tracking, per-note BYPASS (right-click a note), Instrument range.
- **Colour**: Air and Drive; vibrato shapes (Sine / Triangle / Saw / Wander).
- **28 presets**, including *Anime Vocal* (+ Soft / Hyper / Idol), Chipmunk, Giant, Robot Choir, Gospel Choir and more.
- Preset browser (categories, search, favourites, stars) with your own saved presets (category folders; DELETE / OPEN FOLDER / RESCAN like in NyashkSynth); module switches (Formant / Voices / Colour / Vibrato) that skip what is off.
- Voices stay locked to real time at any interval (up to +/-24 st) and their sum is level-compensated with a soft safety, so loud harmony stacks no longer clip.
- New tabbed GUI (TUNE / FORMANT / VOICES).

Downloads: `NyashkTune-<OS>-VST3.zip`. Unzip and copy `NyashkTune.vst3` into your VST3 folder (Linux `~/.vst3/`, Windows `C:\\Program Files\\Common Files\\VST3\\`, macOS `~/Library/Audio/Plug-Ins/VST3/`; macOS builds are unsigned: `xattr -dr com.apple.quarantine ~/Library/Audio/Plug-Ins/VST3/NyashkTune.vst3`).
