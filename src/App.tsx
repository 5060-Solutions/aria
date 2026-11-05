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
import { allowLocalSetupLinks, isProvisionLink, parseProvisionLink } from "./utils/provisionLink";
import { ProvisionLinkDialog } from "./components/provision/ProvisionLinkDialog";
import { provisionFlow } from "./components/provision/provisionApi";
import "./i18n";

const isDebugWindow = getCurrentWebviewWindow().label === "debug";

/** How long a runtime copy of the launch link is treated as a duplicate. */
const LAUNCH_DUPLICATE_MS = 5000;

/**
 * The links Aria was launched with. Read once at load, not per effect run:
 * the backend hands them out only once per run, and a StrictMode re-run must
 * not lose them.
 */
const launchLinks: Promise<string[]> = isDebugWindow
  ? Promise.resolve([])
  : invoke<string[]>("take_launch_links").catch((e) => {
      log.warn("[DeepLink] Could not read the launch link:", e);
      return [];
    });

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

  // Handle tel:, sip: and aria://provision deep links
  useEffect(() => {
    const handle = (urls: string[]) => {
      for (const url of urls) {
        // aria://provision?v=1&url=… — a one-click setup link. It only opens
        // a confirm dialog; nothing is added until the user says so. Neither
        // the link nor its URL is logged: the URL carries a one-time token.
        if (isProvisionLink(url)) {
          const parsed = parseProvisionLink(url, { allowLocalHttp: allowLocalSetupLinks });
          if (!parsed.ok) {
            log.warn("[DeepLink] Ignored a setup link:", parsed.reason);
          } else if (provisionFlow.getState().open(parsed.link)) {
            log.info("[DeepLink] Setup link from", parsed.link.serverHost);
          } else {
            log.info("[DeepLink] Ignored a setup link: another one is open");
          }
          continue;
        }
        // tel:+1234567890 or sip:user@domain.com. The link comes from any web
        // page, so it is only ever put in the dialer, never dialled.
        const target = deepLinkTarget(url);
        if (target === null) continue;
        setDialInput(target);
        setCurrentView("dialer");
        log.info("[DeepLink] Received link, setting dialer to:", target);
      }
    };

    // The launch link can arrive both ways, in either order: from the
    // backend (asked once per run, so a webview reload does not replay it)
    // and as a runtime event. Each copy is handled once.
    const launchHandled = new Set<string>();
    const timers: ReturnType<typeof setTimeout>[] = [];
    const seenBeforeLaunch = new Set<string>();
    let launchRead = false;
    const unlisten = onOpenUrl((urls) => {
      const fresh = urls.filter((u) => !launchHandled.delete(u));
      if (!launchRead) fresh.forEach((u) => seenBeforeLaunch.add(u));
      handle(fresh);
    });

    let cancelled = false;
    void launchLinks.then((urls) => {
      launchRead = true;
      if (cancelled) return;
      const fresh = urls.filter((u) => !seenBeforeLaunch.has(u));
      fresh.forEach((u) => {
        launchHandled.add(u);
        // The runtime copy, if any, comes at once. After that an identical
        // link is the user clicking again and must go through.
        timers.push(setTimeout(() => launchHandled.delete(u), LAUNCH_DUPLICATE_MS));
      });
      handle(fresh);
    });

    return () => {
      cancelled = true;
      timers.forEach(clearTimeout);
      unlisten.then((fn) => fn());
    };
  }, [setDialInput, setCurrentView]);

  return (
    <>
      {setupComplete ? <AppShell /> : <SetupWizard />}
      {/* Outside both, so a setup link works on first run too. */}
      <ProvisionLinkDialog />
    </>
  );
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
