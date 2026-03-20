import { useMemo, useEffect } from "react";
import { ThemeProvider, CssBaseline } from "@mui/material";
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

  // Handle tel: and sip: deep links
  useEffect(() => {
    const unlisten = onOpenUrl((urls) => {
      for (const url of urls) {
        // Handle tel: links (e.g., tel:+1234567890)
        if (url.startsWith("tel:")) {
          const number = decodeURIComponent(url.slice(4));
          setDialInput(number);
          setCurrentView("dialer");
          log.info("[DeepLink] Received tel: link, setting dialer to:", number);
        }
        // Handle sip: links (e.g., sip:user@domain.com)
        else if (url.startsWith("sip:")) {
          const uri = decodeURIComponent(url.slice(4));
          setDialInput(uri);
          setCurrentView("dialer");
          log.info("[DeepLink] Received sip: link, setting dialer to:", uri);
        }
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
