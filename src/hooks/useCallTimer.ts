import { useEffect, useReducer } from "react";

function formatElapsed(connectTime: number): string {
  const seconds = Math.floor((Date.now() - connectTime) / 1000);
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  const h = Math.floor(m / 60);
  if (h > 0) {
    return `${h}:${String(m % 60).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  }
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

export function useCallTimer(connectTime: number | null): string {
  const [, forceRender] = useReducer((x: number) => x + 1, 0);

  useEffect(() => {
    if (!connectTime) return;
    const interval = setInterval(forceRender, 1000);
    return () => clearInterval(interval);
  }, [connectTime]);

  if (!connectTime) return "00:00";
  return formatElapsed(connectTime);
}
