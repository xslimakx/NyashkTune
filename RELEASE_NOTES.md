## NyashkTune v1.1.0

### New in this build
- **ULTRA**: a switch and an amount knob (TRACKING panel). At 100 % the voice sits exactly on the note and steps to the next one instantly, like a MIDI note: no glide, no vibrato, no drift, even with slow Retune settings. Switch it off and everything is as before. New preset *ULTRA Hard Steps*.
- **Steadier tracking**: the note is chosen from a lightly smoothed pitch and has to hold for a moment before the tuner moves, so a voice sitting between two notes, a crack or a scoop no longer flickers between neighbours. Big jumps are still taken at once.
- **Flats**: every black key shows both spellings (C♯ / D♭); the ♯ / ♭ switch on the pitch display chooses which one is used everywhere (display, note names, key list). The host also shows "C# / Db".
- Steadier pitch display (the lines are median-filtered, single-frame spikes are gone).
- **Fixed:** *Voice 1 / 2 On* shared a parameter ID with *Custom Note C / C♯*, so the host and saved projects mixed the two up (found by the Steinberg VST3 validator). The voice switches have their own ID now; in projects saved with older versions the two voice switches start off.

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
