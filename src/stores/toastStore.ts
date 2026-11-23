import { create } from "zustand";

export type ToastSeverity = "error" | "warning" | "info" | "success";

export interface Toast {
  id: number;
  message: string;
  severity: ToastSeverity;
}

interface ToastState {
  /** The toast on screen, if any. */
  current: Toast | null;
  /** Toasts waiting their turn, oldest first. */
  queue: Toast[];
  show: (message: string, severity?: ToastSeverity) => void;
  /** Close the current toast and bring on the next. */
  dismiss: () => void;
}

let nextId = 1;

/**
 * The app's one place to tell the user something went wrong away from a
 * form. Call actions used to fail silently: a rejected transfer, a call that
 * could not be placed, a hold that never reached the far end.
 */
export const useToastStore = create<ToastState>((set) => ({
  current: null,
  queue: [],
  show: (message, severity = "error") =>
    set((s) => {
      // The same message twice in a row (a double click, a repeated failure)
      // is one toast.
      const last = s.queue.length > 0 ? s.queue[s.queue.length - 1] : s.current;
      if (last && last.message === message && last.severity === severity) return s;
      const toast = { id: nextId++, message, severity };
      return s.current ? { queue: [...s.queue, toast] } : { current: toast };
    }),
  dismiss: () =>
    set((s) => ({
      current: s.queue[0] ?? null,
      queue: s.queue.slice(1),
    })),
}));

/** Show an error toast. Usable outside React. */
export function notifyError(message: string): void {
  useToastStore.getState().show(message, "error");
}

/** The text of a rejected promise: backend commands reject with a string. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
