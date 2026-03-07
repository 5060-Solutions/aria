import { useEffect, useRef } from "react";

/**
 * Aria ringtone — synthesized via Web Audio API, no audio files required.
 *
 * Pattern: A rising A-major arpeggio (A4 → C#5 → E5) followed by a brief
 * descending answer (E5 → C#5), looping with a rest between each burst.
 * Gives a clean, melodic character fitting the "Aria" brand.
 */

const NOTES = [
  { freq: 440.0, dur: 0.12 }, // A4
  { freq: 554.37, dur: 0.12 }, // C#5
  { freq: 659.26, dur: 0.18 }, // E5  (held slightly longer)
  { freq: 554.37, dur: 0.12 }, // C#5 (descend)
  { freq: 440.0, dur: 0.18 }, // A4  (resolve)
];

const GAP = 0.04; // seconds between notes
const REST = 1.2; // seconds of silence between bursts

function burstDuration(): number {
  return NOTES.reduce((acc, n) => acc + n.dur + GAP, 0);
}

function scheduleBurst(ctx: AudioContext, masterGain: GainNode, startTime: number) {
  let t = startTime;

  for (const note of NOTES) {
    const osc = ctx.createOscillator();
    const env = ctx.createGain();

    osc.type = "sine";
    osc.frequency.setValueAtTime(note.freq, t);

    // Slight pitch shimmer: second oscillator a few cents sharp for richness
    const osc2 = ctx.createOscillator();
    const env2 = ctx.createGain();
    osc2.type = "sine";
    osc2.frequency.setValueAtTime(note.freq * 1.004, t);
    env2.gain.setValueAtTime(0, t);
    env2.gain.linearRampToValueAtTime(0.25, t + 0.012);
    env2.gain.setValueAtTime(0.25, t + note.dur - 0.02);
    env2.gain.linearRampToValueAtTime(0, t + note.dur);
    osc2.connect(env2);
    env2.connect(masterGain);
    osc2.start(t);
    osc2.stop(t + note.dur + 0.01);

    // Main envelope: fast attack, sustain, soft release
    env.gain.setValueAtTime(0, t);
    env.gain.linearRampToValueAtTime(0.55, t + 0.015);
    env.gain.setValueAtTime(0.55, t + note.dur - 0.025);
    env.gain.linearRampToValueAtTime(0, t + note.dur);

    osc.connect(env);
    env.connect(masterGain);
    osc.start(t);
    osc.stop(t + note.dur + 0.01);

    t += note.dur + GAP;
  }
}

export function useRingtone(active: boolean) {
  const ctxRef = useRef<AudioContext | null>(null);
  const loopRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const masterRef = useRef<GainNode | null>(null);

  useEffect(() => {
    if (!active) {
      // Fade out and stop
      if (masterRef.current && ctxRef.current) {
        masterRef.current.gain.linearRampToValueAtTime(
          0,
          ctxRef.current.currentTime + 0.1,
        );
      }
      if (loopRef.current) {
        clearTimeout(loopRef.current);
        loopRef.current = null;
      }
      // Close context after fade
      const ctx = ctxRef.current;
      if (ctx) {
        setTimeout(() => ctx.close(), 200);
        ctxRef.current = null;
        masterRef.current = null;
      }
      return;
    }

    // Create a fresh audio context each time ringing starts
    const ctx = new AudioContext();
    ctxRef.current = ctx;

    const master = ctx.createGain();
    master.gain.setValueAtTime(0.7, ctx.currentTime);
    master.connect(ctx.destination);
    masterRef.current = master;

    const burst = burstDuration();
    const period = burst + REST;

    function loop() {
      if (!ctxRef.current) return;
      scheduleBurst(ctx, master, ctx.currentTime);
      loopRef.current = setTimeout(loop, period * 1000);
    }

    loop();

    return () => {
      if (loopRef.current) clearTimeout(loopRef.current);
      master.gain.linearRampToValueAtTime(0, ctx.currentTime + 0.08);
      setTimeout(() => ctx.close(), 150);
      ctxRef.current = null;
      masterRef.current = null;
    };
  }, [active]);
}
