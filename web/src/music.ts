/** Scientific pitch names; MIDI 60 is C4. */
export function noteName(pitch: number) { return ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'][pitch % 12] + String(Math.floor(pitch / 12) - 1); }
