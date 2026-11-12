## NyashkTune v1.1.2

- **Alt-tab fix (Linux):** after alt-tab, minimising or covering the window, parts of the picture (backgrounds, knobs, images) could stay blank until something on them moved. The window is now drawn again in full whenever the system throws its contents away, and pictures are no longer lost while the window is hidden or a frame is skipped.
- Smoother images in the CPU renderer (used when Vulkan is not available): pictures drawn smaller than their real size, such as knobs and photos, are now scaled with mipmaps instead of picking single pixels, so they no longer look grainy or jagged.
- **Display fix:** the built-in CPU renderer (used when Vulkan is not available) drew pixels on the shared edges of triangles twice, which showed as brighter seams and specks in see-through fills (spectra, curves, bars). It now uses an exact fill rule like a GPU: every pixel is drawn once, without holes.
- Pitch display: note names at the very top and bottom edge are no longer cut in half.
- The second spelling under the black keys (D♭, E♭ ...) is easier to read.
- Transpose, voice intervals, Air and voice levels no longer show "-0".
- Value boxes have a small gap between them, and a value that would not fit its box is drawn smaller instead of running over the frame.
- Clicking a value box and pressing Enter no longer changes a value whose unit matters (for example "1.03 s" was read back as 1.03 ms).
- New automatic test: every parameter's text is typed back over its whole range and must give the same value.

## NyashkTune v1.1.1

- **Voice range no longer caps high notes.** The range now sets only the lowest note (and the latency); upwards the tuner always searches to at least about C6. Before, with *Bass* (top ~F4) a voice going above it was read an octave too low and the chosen note jumped while gliding through F4 (test: C4 -> C5 glide with Bass, 878 blocks on a wrong note before, 0 now). Low bass notes are still found without octave errors.
- **Switching ULTRA off brings back the normal sound at once** (tested: vibrato is back to the same amount as with ULTRA never used). The *ULTRA Hard Steps* preset no longer sets Retune and Transition to 0 ms, so after switching ULTRA off you are not left with a hard 0 ms tune.

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
