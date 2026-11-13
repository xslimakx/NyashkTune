## NyashkTune 2.0.0

Rebuilt around the controls and the logic of a classic real-time tuner. Everything else is gone.

- **Controls:** Speed (default 15 ms) and Note Transition (120 ms) with Link; **Tolerance** in cents (Off–40) and time (Off–300 ms); **Vibrato** On / Off with **Depth** 0–200 %; **Correction** On / Off and amount; **Formant** Corrected / Non-corrected; Reference ±100 cents; Root, Scale, Reset Scale, Voice range, MIDI target.
- **Keyboard** with note states for every note: Legal, Illegal, Illegal – pitch up, Illegal – pitch down, Bypass; the correction **range** is dragged on the keyboard, notes outside it pass uncorrected.
- **Natural by default:** with Vibrato On only the note centre is corrected and the singer's own vibrato is kept (or flattened / widened with Depth). The note is always chosen on the vibrato-free centre, so a wide vibrato never flips it to a neighbour - in either mode.
- **Cleaner sound:** the resynthesis lays every grain at the real distance to the next pitch mark. Before, grains drifted against the original even with no correction (periods dropped / repeated, a rougher sound); with no correction the voice now comes out as it went in (~50 dB instead of ~28 dB match).
- **Removed:** ULTRA, harmony voices, Air / Drive, the synthetic vibrato, formant shift and its algorithms, Flex-Tune, Humanize, Transpose / Detune, Mix, Gate, input / output gain, the preset browser and the pitch graph. New correction meter (arc, cents) with the target note, IN / OUT meters.
- Old projects: Root, Scale, Speed, Note Transition, Link, Amount, Reference and MIDI target load; the rest starts from the new defaults.

## NyashkTune 1.1.4

- **No more horrible jumps and broken sound.** On a fast slide of the voice, after a leap, or when the pitch detector slipped an octave, the tuner kept the old note for tens of milliseconds and pulled the voice 2-5 semitones away, sometimes a whole octave (12 st). With Retune 0 this was heard as a gargled, broken sound and as the tall spikes on the display. Now the tuner never pulls the voice further than its nearest note for more than ~12 ms, and an octave slip costs nothing (the correction never moves the voice by a whole octave).
- Checked on real a cappella and speech recordings, not only on synthetic voices: before, the voice was pulled beyond its nearest note for up to 145 ms at a time (up to 12 st); now for at most 12 ms. Detection stays within ~5 cents of a reference pitch tracker (pYIN) on singing, with almost no octave errors.
- New regression tests for this (`never_pulls_far_from_the_nearest_note`, and `real_voice_never_pulls_far` on your own recordings via `NYASHK_VOICE_DIR`), plus an offline tool `examples/run_raw.rs` to run the engine over a recording and log what it does.

## NyashkTune 1.1.3

- **Notes no longer get missed at high Tracking.** A note sung a little off (25-40 cents) right after a step could stay on the previous note for the whole held note when Tracking was high. Tracking now only delays a switch for a moment; if your voice stays nearer to another note, the tuner always moves to it.
- **Cleaner transitions.** On a jump of about a semitone or more the tuner used to drag through the notes in between and could settle on one of them (61 -> 59 landing on 60). It now goes straight to the new note.
- **Faster landing on the new note:** about 45-55 ms at the default Tracking (it was up to 250 ms at Tracking 100 %, now about 100-150 ms there).
- **ULTRA steps on time.** ULTRA no longer slows down the note choice, and a wide vibrato no longer flips it to the neighbouring note (it stays within 2 cents of the note).
- **Octave leaps.** After a leap up of about an octave the pitch detector could stay an octave low for the whole note (wrong octave on screen, rougher shifting). It now follows the leap.
- New stress tests: hundreds of sung transitions (jumps, glides, scoops, off-pitch notes with vibrato; chromatic and minor/phrygian scales; every Tracking / ULTRA / Retune setting) must never pick a note outside the jump, never show steps on a clean jump, land quickly and sit on the note.

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
