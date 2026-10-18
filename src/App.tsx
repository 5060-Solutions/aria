import { useMemo, useEffect } from "react";
import { ThemeProvider, CssBaseline } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { useAppStore } from "./stores/appStore";
import { lightTheme, darkTheme } from "./theme";
import { AppShell } from "./components/layout/AppShell";
import { SetupWizard } from "./components/wizard/SetupWizard";
import { DiagnosticPanel } from "./components/diagnostics/DiagnosticPanel";
import { useSipEvents, useAutoRegister } from "./hooks/useSip";
import { useNetworkMonitor } from "./hooks/useNetworkMonitor";
import { log } from "./utils/log";
import { deepLinkTarget } from "./utils/deepLink";
import "./i18n";

const isDebugWindow = getCurrentWebviewWindow().label === "debug";

/** Main app — only rendered in the primary window. Holds all SIP hooks. */
function MainApp() {
  const setupComplete = useAppStore((s) => s.setupComplete);
  const setDialInput = useAppStore((s) => s.setDialInput);
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  useSipEvents();
  useAutoRegister();
  useNetworkMonitor();

  // The engine's audio processing setting lives in memory and resets with the
  // process, so the saved preference has to be pushed down on every launch.
  // Without this, echo cancellation would silently be off until the user
  // opened Settings and toggled something.
  //
  // Read imperatively rather than subscribing: later changes are pushed by the
  // Settings screen itself, and subscribing here would send each one twice.
  useEffect(() => {
    const settings = useAppStore.getState().voiceProcessing;
    invoke("set_voice_processing", { settings }).catch((e) => {
      log.warn("[Audio] Could not apply voice processing settings:", e);
    });
  }, []);

  // Handle tel: and sip: deep links
  useEffect(() => {
    const unlisten = onOpenUrl((urls) => {
      for (const url of urls) {
        // tel:+1234567890 or sip:user@domain.com. The link comes from any web
        // page, so it is only ever put in the dialer, never dialled.
        const target = deepLinkTarget(url);
        if (target === null) continue;
        setDialInput(target);
        setCurrentView("dialer");
        log.info("[DeepLink] Received link, setting dialer to:", target);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [setDialInput, setCurrentView]);

  return setupComplete ? <AppShell /> : <SetupWizard />;
}

export function App() {
  const darkMode = useAppStore((s) => s.darkMode);
  const theme = useMemo(() => (darkMode ? darkTheme : lightTheme), [darkMode]);

  return (
    <ThemeProvider theme={theme}>
      <CssBaseline />
      {isDebugWindow ? <DiagnosticPanel isDetached /> : <MainApp />}
    </ThemeProvider>
  );
}
