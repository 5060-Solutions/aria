import { Box } from "@mui/material";
import { AnimatePresence, motion } from "framer-motion";
import { useAppStore } from "../../stores/appStore";
import { useKeyboardShortcuts } from "../../hooks/useKeyboardShortcuts";
import { NavRail } from "./NavRail";
import { StatusBar } from "./StatusBar";
import { Dialer } from "../dialer/Dialer";
import { CallScreen } from "../call/CallScreen";
import { CallHistory } from "../history/CallHistory";
import { ContactList } from "../contacts/ContactList";
import { Settings } from "../settings/Settings";
import { DiagnosticPanel } from "../diagnostics/DiagnosticPanel";

const views = {
  dialer: Dialer,
  history: CallHistory,
  contacts: ContactList,
  settings: Settings,
  diagnostics: DiagnosticPanel,
};

export function AppShell() {
  useKeyboardShortcuts();
  
  const currentView = useAppStore((s) => s.currentView);
  const activeCall = useAppStore((s) => s.activeCall);

  const ViewComponent = views[currentView];
  const showCallScreen =
    activeCall && activeCall.state !== "idle" && activeCall.state !== "ended";

  return (
    <Box
      sx={{
        display: "flex",
        height: "100vh",
        bgcolor: "background.default",
        overflow: "hidden",
      }}
    >
      <NavRail />
      <Box
        sx={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
        }}
      >
        <StatusBar />
        <Box sx={{ flex: 1, overflow: "auto", position: "relative" }}>
          <AnimatePresence mode="wait">
            {showCallScreen ? (
              <motion.div
                key="call"
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -20 }}
                transition={{ duration: 0.25, ease: "easeOut" }}
                style={{ height: "100%" }}
              >
                <CallScreen />
              </motion.div>
            ) : (
              <motion.div
                key={currentView}
                initial={{ opacity: 0, x: 10 }}
                animate={{ opacity: 1, x: 0 }}
                exit={{ opacity: 0, x: -10 }}
                transition={{ duration: 0.2, ease: "easeOut" }}
                style={{ height: "100%" }}
              >
                <ViewComponent />
              </motion.div>
            )}
          </AnimatePresence>
        </Box>
      </Box>
    </Box>
  );
}
