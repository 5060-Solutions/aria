const DTMF_FREQUENCIES: Record<string, [number, number]> = {
  "1": [697, 1209],
  "2": [697, 1336],
  "3": [697, 1477],
  "4": [770, 1209],
  "5": [770, 1336],
  "6": [770, 1477],
  "7": [852, 1209],
  "8": [852, 1336],
  "9": [852, 1477],
  "*": [941, 1209],
  "0": [941, 1336],
  "#": [941, 1477],
};

let audioContext: AudioContext | null = null;

function getAudioContext(): AudioContext {
  if (!audioContext) {
    audioContext = new AudioContext();
  }
  return audioContext;
}

export function playDtmfTone(digit: string, durationMs = 120): void {
  const freqs = DTMF_FREQUENCIES[digit];
  if (!freqs) return;

  const ctx = getAudioContext();
  const now = ctx.currentTime;
  const duration = durationMs / 1000;

  const gainNode = ctx.createGain();
  gainNode.connect(ctx.destination);
  gainNode.gain.setValueAtTime(0.15, now);
  gainNode.gain.exponentialRampToValueAtTime(0.001, now + duration);

  const [low, high] = freqs;

  const osc1 = ctx.createOscillator();
  osc1.type = "sine";
  osc1.frequency.setValueAtTime(low, now);
  osc1.connect(gainNode);
  osc1.start(now);
  osc1.stop(now + duration);

  const osc2 = ctx.createOscillator();
  osc2.type = "sine";
  osc2.frequency.setValueAtTime(high, now);
  osc2.connect(gainNode);
  osc2.start(now);
  osc2.stop(now + duration);
}
