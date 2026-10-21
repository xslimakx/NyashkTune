## NyashkTune v1.0.0

NyashkTune is a realtime vocal pitch correction (autotune) VST3 plugin.

### New in this build
- **Selectable formant algorithm**: Classic, Smooth, Detailed, Vocal (LPC), Bend - plus Tilt and Follow, and a live formant map.
- **2 extra voices** (harmony / octave / doubler) with scale-aware intervals, level, pan, detune, formant and delay.
- **Antares-style extras**: Tracking, per-note BYPASS (right-click a note), Scale Learn (pitch histogram, DETECT KEY, LEARN into Custom), Instrument range.
- **Colour**: Air and Drive; vibrato shapes (Sine / Triangle / Saw / Wander).
- **28 presets**, including *Anime Vocal* (+ Soft / Hyper / Idol), Chipmunk, Giant, Robot Choir, Gospel Choir and more.
- New tabbed GUI (TUNE / FORMANT / VOICES).

Downloads: `NyashkTune-<OS>-VST3.zip`. Unzip and copy `NyashkTune.vst3` into your VST3 folder (Linux `~/.vst3/`, Windows `C:\\Program Files\\Common Files\\VST3\\`, macOS `~/Library/Audio/Plug-Ins/VST3/`; macOS builds are unsigned: `xattr -dr com.apple.quarantine ~/Library/Audio/Plug-Ins/VST3/NyashkTune.vst3`).
