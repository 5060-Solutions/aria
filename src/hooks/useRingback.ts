import { useEffect, useRef } from "react";

/**
 * North American ringback tone: 440 Hz + 480 Hz, 2 seconds on, 4 seconds off.
 * Played locally when an outbound call is ringing (180/183 without early media).
 */

const FREQ_LOW = 440;
const FREQ_HIGH = 480;
const ON_DURATION = 2; // seconds
const OFF_DURATION = 4; // seconds
const CYCLE = ON_DURATION + OFF_DURATION;

export function useRingback(active: boolean) {
  const ctxRef = useRef<AudioContext | null>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    if (!active) {
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
      const ctx = ctxRef.current;
      if (ctx) {
        ctx.close();
        ctxRef.current = null;
      }
      return;
    }

    const ctx = new AudioContext();
    ctxRef.current = ctx;

    function playBurst() {
      if (!ctxRef.current || ctxRef.current.state === "closed") return;
      const c = ctxRef.current;
      const now = c.currentTime;

      const gain = c.createGain();
      gain.connect(c.destination);
      gain.gain.setValueAtTime(0.15, now);
      gain.gain.setValueAtTime(0.15, now + ON_DURATION - 0.02);
      gain.gain.linearRampToValueAtTime(0, now + ON_DURATION);

      const osc1 = c.createOscillator();
      osc1.type = "sine";
      osc1.frequency.setValueAtTime(FREQ_LOW, now);
      osc1.connect(gain);
      osc1.start(now);
      osc1.stop(now + ON_DURATION);

      const osc2 = c.createOscillator();
      osc2.type = "sine";
      osc2.frequency.setValueAtTime(FREQ_HIGH, now);
      osc2.connect(gain);
      osc2.start(now);
      osc2.stop(now + ON_DURATION);
    }

    // Play immediately, then repeat on cycle
    playBurst();
    intervalRef.current = setInterval(playBurst, CYCLE * 1000);

    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
      ctx.close();
      ctxRef.current = null;
    };
  }, [active]);
}
