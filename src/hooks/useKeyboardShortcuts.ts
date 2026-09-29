import { useEffect, useCallback } from "react";
import { useAppStore } from "../stores/appStore";
import { hangupCall, sipMute, sipHold, sipStartRecording, sipStopRecording, sipAnswer } from "./useSip";
import { log } from "../utils/log";

/**
 * Global keyboard shortcuts for Aria
 *
 * ⌘/Ctrl + D - Open dialer
 * ⌘/Ctrl + Enter - Make/answer call
 * ⌘/Ctrl + K - End call
 * ⌘/Ctrl + M - Toggle mute
 * ⌘/Ctrl + H - Toggle hold
 * ⌘/Ctrl + R - Toggle recording
 * ⌘/Ctrl + , - Open settings
 */
export function useKeyboardShortcuts() {
  const setCurrentView = useAppStore((s) => s.setCurrentView);

  const handleHangup = useCallback(async () => {
    const call = useAppStore.getState().activeCall;
    if (!call || call.state === "idle" || call.state === "ended") return;

    await hangupCall(call);
  }, []);

  const handleToggleMute = useCallback(async () => {
    const call = useAppStore.getState().activeCall;
    if (!call || call.state !== "connected") return;

    useAppStore.getState().toggleMute();
    try {
      await sipMute(call.id, !call.muted);
    } catch {
      // ignore
    }
  }, []);

  const handleToggleHold = useCallback(async () => {
    const call = useAppStore.getState().activeCall;
    if (!call || call.state !== "connected") return;

    useAppStore.getState().toggleHold();
    try {
      await sipHold(call.id, !call.held);
    } catch {
      // ignore
    }
  }, []);

  const handleToggleRecording = useCallback(async () => {
    const call = useAppStore.getState().activeCall;
    if (!call || call.state !== "connected") return;

    try {
      if (call.recording) {
        await sipStopRecording(call.id);
        useAppStore.getState().setCallRecording(call.id, false);
      } else {
        const path = await sipStartRecording(call.id);
        useAppStore.getState().setCallRecording(call.id, true, path);
      }
    } catch (e) {
      log.error("Recording toggle failed:", e);
    }
  }, []);

  const handleMakeOrAnswerCall = useCallback(async () => {
    const call = useAppStore.getState().activeCall;
    
    // If there's an incoming call, answer it
    if (call?.state === "incoming") {
      try {
        await sipAnswer(call.id);
      } catch (e) {
        log.error("Failed to answer call:", e);
      }
      return;
    }

    // If already in a call, do nothing
    if (call && call.state !== "idle" && call.state !== "ended") return;

    // Otherwise, focus on dialer (the user can then press Enter to dial)
    useAppStore.getState().setCurrentView("dialer");
  }, []);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Check for Cmd (Mac) or Ctrl (Windows/Linux)
      const isMod = e.metaKey || e.ctrlKey;
      if (!isMod) return;

      // Don't trigger shortcuts when typing in an input field
      const target = e.target as HTMLElement;
      if (
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.isContentEditable
      ) {
        // Allow ⌘K and ⌘Enter even in inputs (for call control)
        if (e.key !== "k" && e.key !== "Enter") {
          return;
        }
      }

      switch (e.key.toLowerCase()) {
        case "d":
          e.preventDefault();
          setCurrentView("dialer");
          break;

        case ",":
          e.preventDefault();
          setCurrentView("settings");
          break;

        case "k":
          e.preventDefault();
          handleHangup();
          break;

        case "m":
          e.preventDefault();
          handleToggleMute();
          break;

        case "h":
          e.preventDefault();
          handleToggleHold();
          break;

        case "r":
          e.preventDefault();
          handleToggleRecording();
          break;

        case "enter":
          e.preventDefault();
          handleMakeOrAnswerCall();
          break;
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [
    setCurrentView,
    handleHangup,
    handleToggleMute,
    handleToggleHold,
    handleToggleRecording,
    handleMakeOrAnswerCall,
  ]);
}
