import { useState } from "react";
import { Box, IconButton, alpha, useTheme, Typography, Dialog, DialogTitle, DialogContent, TextField, Button } from "@mui/material";
import MicIcon from "@mui/icons-material/Mic";
import MicOffIcon from "@mui/icons-material/MicOff";
import PauseIcon from "@mui/icons-material/Pause";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import CallEndIcon from "@mui/icons-material/CallEnd";
import DialpadIcon from "@mui/icons-material/Dialpad";
import VolumeUpIcon from "@mui/icons-material/VolumeUp";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import StopIcon from "@mui/icons-material/Stop";
import AddIcCallIcon from "@mui/icons-material/AddIcCall";
import CallMergeIcon from "@mui/icons-material/CallMerge";
import SwapCallsIcon from "@mui/icons-material/SwapCalls";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { sipHangup, sipMute, sipHold, sipStartRecording, sipStopRecording, sipAddCall, sipConferenceMerge, sipSwapCalls } from "../../hooks/useSip";
import { log } from "../../utils/log";

interface ControlButtonProps {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  onClick: () => void;
}

function ControlButton({ icon, label, active, onClick }: ControlButtonProps) {
  const theme = useTheme();
  return (
    <Box
      sx={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 0.5,
      }}
    >
      <motion.div whileTap={{ scale: 0.88 }}>
        <IconButton
          onClick={onClick}
          sx={{
            width: 56,
            height: 56,
            borderRadius: "50%",
            bgcolor: active
              ? "primary.main"
              : alpha(theme.palette.text.primary, 0.06),
            color: active ? "primary.contrastText" : "text.primary",
            transition: "all 0.15s ease",
            "&:hover": {
              bgcolor: active
                ? "primary.dark"
                : alpha(theme.palette.text.primary, 0.1),
            },
          }}
        >
          {icon}
        </IconButton>
      </motion.div>
      <Typography
        variant="caption"
        sx={{ fontSize: "0.65rem", color: "text.secondary" }}
      >
        {label}
      </Typography>
    </Box>
  );
}

export function CallControls() {
  const { t } = useTranslation();
  const activeCall = useAppStore((s) => s.activeCall);
  const activeCalls = useAppStore((s) => s.activeCalls);
  const conferences = useAppStore((s) => s.conferences);
  const toggleMute = useAppStore((s) => s.toggleMute);
  const toggleHold = useAppStore((s) => s.toggleHold);
  const setActiveCall = useAppStore((s) => s.setActiveCall);
  const updateCall = useAppStore((s) => s.updateCall);
  const setPrimaryCall = useAppStore((s) => s.setPrimaryCall);
  const createConference = useAppStore((s) => s.createConference);
  const addCallHistory = useAppStore((s) => s.addCallHistory);
  
  const [addCallDialogOpen, setAddCallDialogOpen] = useState(false);
  const [newCallUri, setNewCallUri] = useState("");

  if (!activeCall) return null;
  
  // Check if we have multiple calls for conference features
  const connectedCalls = activeCalls.filter(
    (c) => c.state === "connected" || c.state === "held" || c.state === "conferenced"
  );
  const heldCalls = activeCalls.filter((c) => c.state === "held");
  const hasMultipleCalls = connectedCalls.length > 1;
  const isInConference = activeCall.conferenceId !== undefined;
  const canAddCall = activeCall.state === "connected" && !isInConference;
  const canMerge = hasMultipleCalls && !isInConference;

  const handleHangup = async () => {
    // If recording is active, stop it and get the path
    let recordingPath = activeCall.recordingPath;
    if (activeCall.recording) {
      try {
        const path = await sipStopRecording(activeCall.id);
        if (path) recordingPath = path;
      } catch {
        // Recording stop failed, but continue with hangup
      }
    }

    try {
      await sipHangup(activeCall.id);
    } catch {
      // Fallback: end locally even if backend fails
    }

    const endTime = Date.now();
    const duration = activeCall.connectTime
      ? Math.floor((endTime - activeCall.connectTime) / 1000)
      : 0;

    addCallHistory({
      id: activeCall.id,
      accountId: activeCall.accountId,
      remoteUri: activeCall.remoteUri,
      remoteName: activeCall.remoteName,
      direction: activeCall.direction,
      startTime: activeCall.startTime ?? endTime,
      duration,
      missed: !activeCall.connectTime,
      recordingPath,
    });

    setActiveCall({ ...activeCall, state: "ended", endTime });
    setTimeout(() => setActiveCall(null), 1200);
  };

  const handleMute = async () => {
    toggleMute();
    try {
      await sipMute(activeCall.id, !activeCall.muted);
    } catch {
      // ignore
    }
  };

  const handleHold = async () => {
    toggleHold();
    try {
      await sipHold(activeCall.id, !activeCall.held);
    } catch {
      // ignore
    }
  };

  const handleRecord = async () => {
    try {
      if (activeCall.recording) {
        await sipStopRecording(activeCall.id);
        // Clear recording state but keep the existing path
        setActiveCall({ ...activeCall, recording: false });
      } else {
        const path = await sipStartRecording(activeCall.id);
        // Store the recording path
        setActiveCall({ ...activeCall, recording: true, recordingPath: path });
      }
    } catch (e) {
      log.error("Recording toggle failed:", e);
    }
  };
  
  const handleAddCall = async () => {
    if (!newCallUri.trim()) return;
    
    try {
      // Put current call on hold first
      await sipHold(activeCall.id, true);
      updateCall(activeCall.id, { held: true, state: "held" });
      
      // Make the new call
      const newCallId = await sipAddCall(newCallUri.trim());
      
      // The new call will be added to activeCalls via SIP event
      setPrimaryCall(newCallId);
      
      setAddCallDialogOpen(false);
      setNewCallUri("");
    } catch (e) {
      log.error("Add call failed:", e);
    }
  };
  
  const handleMerge = async () => {
    const callIds = connectedCalls.map((c) => c.id);
    if (callIds.length < 2) return;
    
    try {
      const conferenceId = await sipConferenceMerge(callIds);
      // Update local state
      createConference(callIds);
      log.info("Conference created:", conferenceId);
    } catch (e) {
      log.error("Merge failed:", e);
    }
  };
  
  const handleSwap = async () => {
    // Find the other call (held one)
    const otherCall = heldCalls.find((c) => c.id !== activeCall.id);
    if (!otherCall) return;
    
    try {
      await sipSwapCalls(activeCall.id, otherCall.id);
      
      // Update local state
      updateCall(activeCall.id, { held: true, state: "held" });
      updateCall(otherCall.id, { held: false, state: "connected" });
      setPrimaryCall(otherCall.id);
    } catch (e) {
      log.error("Swap failed:", e);
    }
  };

  return (
    <>
      <Box sx={{ display: "flex", flexDirection: "column", gap: 3, mb: 1 }}>
        {/* Main controls row */}
        <Box
          sx={{
            display: "grid",
            gridTemplateColumns: "repeat(3, 1fr)",
            gap: 2.5,
            justifyItems: "center",
          }}
        >
          <ControlButton
            icon={activeCall.muted ? <MicOffIcon /> : <MicIcon />}
            label={activeCall.muted ? t("call.unmute") : t("call.mute")}
            active={activeCall.muted}
            onClick={handleMute}
          />
          <ControlButton
            icon={<DialpadIcon />}
            label={t("call.keypad")}
            onClick={() => {}}
          />
          <ControlButton
            icon={<VolumeUpIcon />}
            label={t("call.speaker")}
            onClick={() => {}}
          />
          <ControlButton
            icon={activeCall.held ? <PlayArrowIcon /> : <PauseIcon />}
            label={activeCall.held ? t("call.resume") : t("call.hold")}
            active={activeCall.held}
            onClick={handleHold}
          />
          <ControlButton
            icon={activeCall.recording ? <StopIcon /> : <FiberManualRecordIcon />}
            label={activeCall.recording ? t("call.stopRec") : t("call.record")}
            active={activeCall.recording}
            onClick={handleRecord}
          />
          {/* Add Call button - shown when connected and not in conference */}
          {canAddCall && (
            <ControlButton
              icon={<AddIcCallIcon />}
              label={t("call.addCall", "Add Call")}
              onClick={() => setAddCallDialogOpen(true)}
            />
          )}
        </Box>
        
        {/* Conference controls row - shown when multiple calls */}
        {hasMultipleCalls && (
          <Box
            sx={{
              display: "flex",
              justifyContent: "center",
              gap: 2.5,
            }}
          >
            {canMerge && (
              <ControlButton
                icon={<CallMergeIcon />}
                label={t("call.merge", "Merge")}
                onClick={handleMerge}
              />
            )}
            {heldCalls.length > 0 && (
              <ControlButton
                icon={<SwapCallsIcon />}
                label={t("call.swap", "Swap")}
                onClick={handleSwap}
              />
            )}
          </Box>
        )}
        
        {/* Conference indicator */}
        {isInConference && (
          <Box sx={{ textAlign: "center" }}>
            <Typography variant="caption" color="primary">
              {t("call.inConference", "Conference Call")} ({conferences.find(c => c.id === activeCall.conferenceId)?.callIds.length ?? 0} {t("call.participants", "participants")})
            </Typography>
          </Box>
        )}

        <Box sx={{ display: "flex", justifyContent: "center" }}>
          <motion.div whileTap={{ scale: 0.88 }}>
            <IconButton
              onClick={handleHangup}
              sx={{
                width: 68,
                height: 68,
                borderRadius: "50%",
                bgcolor: "error.main",
                color: "#fff",
                boxShadow: (theme) =>
                  `0 6px 24px ${alpha(theme.palette.error.main, 0.4)}`,
                "&:hover": { bgcolor: "error.dark" },
              }}
            >
              <CallEndIcon sx={{ fontSize: 30 }} />
            </IconButton>
          </motion.div>
        </Box>
      </Box>
      
      {/* Add Call Dialog */}
      <Dialog 
        open={addCallDialogOpen} 
        onClose={() => setAddCallDialogOpen(false)}
        maxWidth="xs"
        fullWidth
      >
        <DialogTitle>{t("call.addCall", "Add Call")}</DialogTitle>
        <DialogContent>
          <TextField
            autoFocus
            margin="dense"
            label={t("call.phoneNumber", "Phone Number or SIP URI")}
            fullWidth
            variant="outlined"
            value={newCallUri}
            onChange={(e) => setNewCallUri(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                handleAddCall();
              }
            }}
          />
          <Box sx={{ display: "flex", gap: 1, mt: 2, justifyContent: "flex-end" }}>
            <Button onClick={() => setAddCallDialogOpen(false)}>
              {t("common.cancel", "Cancel")}
            </Button>
            <Button variant="contained" onClick={handleAddCall} disabled={!newCallUri.trim()}>
              {t("call.dial", "Dial")}
            </Button>
          </Box>
        </DialogContent>
      </Dialog>
    </>
  );
}
