import { useState, useEffect } from "react";
import {
  Box, IconButton, alpha, useTheme, Typography, Dialog, DialogTitle,
  DialogContent, TextField, Button, Popover, List, ListItemButton,
  ListItemIcon, ListItemText, Divider,
} from "@mui/material";
import MicIcon from "@mui/icons-material/Mic";
import MicOffIcon from "@mui/icons-material/MicOff";
import PauseIcon from "@mui/icons-material/Pause";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import CallEndIcon from "@mui/icons-material/CallEnd";
import DialpadIcon from "@mui/icons-material/Dialpad";
import HeadsetMicIcon from "@mui/icons-material/HeadsetMic";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import StopIcon from "@mui/icons-material/Stop";
import AddIcCallIcon from "@mui/icons-material/AddIcCall";
import CallMergeIcon from "@mui/icons-material/CallMerge";
import SwapCallsIcon from "@mui/icons-material/SwapCalls";
import CheckIcon from "@mui/icons-material/Check";
import VolumeUpIcon from "@mui/icons-material/VolumeUp";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../../stores/appStore";
import CallIcon from "@mui/icons-material/Call";
import { sipHangup, sipAnswer, sipMute, sipHold, sipStartRecording, sipStopRecording, sipAddCall, sipConferenceMerge, sipSwapCalls, sipSendDtmf } from "../../hooks/useSip";
import { DialerButton } from "../dialer/DialerButton";
import { log } from "../../utils/log";

interface AudioDevice {
  name: string;
  isDefault: boolean;
}

interface AudioDevices {
  inputDevices: AudioDevice[];
  outputDevices: AudioDevice[];
}

interface ControlButtonProps {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  onClick: (e: React.MouseEvent<HTMLButtonElement>) => void;
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

  const selectedInputDevice = useAppStore((s) => s.selectedInputDevice);
  const selectedOutputDevice = useAppStore((s) => s.selectedOutputDevice);
  const setSelectedInputDevice = useAppStore((s) => s.setSelectedInputDevice);
  const setSelectedOutputDevice = useAppStore((s) => s.setSelectedOutputDevice);

  const [addCallDialogOpen, setAddCallDialogOpen] = useState(false);
  const [newCallUri, setNewCallUri] = useState("");
  const [dtmfAnchor, setDtmfAnchor] = useState<HTMLButtonElement | null>(null);
  const [audioAnchor, setAudioAnchor] = useState<HTMLButtonElement | null>(null);
  const [audioDevices, setAudioDevices] = useState<AudioDevices | null>(null);

  // Load audio devices when popover opens
  useEffect(() => {
    if (audioAnchor) {
      invoke<AudioDevices>("get_audio_devices")
        .then(setAudioDevices)
        .catch(() => {});
    }
  }, [audioAnchor]);

  if (!activeCall) return null;

  const isIncoming = activeCall.state === "incoming";

  const handleAnswer = async () => {
    try {
      await sipAnswer(activeCall.id);
    } catch (e) {
      log.error("Answer failed:", e);
    }
  };

  const handleDecline = async () => {
    try {
      await sipHangup(activeCall.id);
    } catch {
      // ignore
    }
    setActiveCall(null);
  };

  if (isIncoming) {
    return (
      <Box sx={{ display: "flex", justifyContent: "center", gap: 6, mb: 1 }}>
        <Box sx={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 0.5 }}>
          <motion.div whileTap={{ scale: 0.88 }}>
            <IconButton
              onClick={handleDecline}
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
          <Typography variant="caption" sx={{ fontSize: "0.7rem", color: "text.secondary" }}>
            {t("call.decline")}
          </Typography>
        </Box>
        <Box sx={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 0.5 }}>
          <motion.div whileTap={{ scale: 0.88 }}>
            <IconButton
              onClick={handleAnswer}
              sx={{
                width: 68,
                height: 68,
                borderRadius: "50%",
                bgcolor: "success.main",
                color: "#fff",
                boxShadow: (theme) =>
                  `0 6px 24px ${alpha(theme.palette.success.main, 0.4)}`,
                "&:hover": { bgcolor: "success.dark" },
              }}
            >
              <CallIcon sx={{ fontSize: 30 }} />
            </IconButton>
          </motion.div>
          <Typography variant="caption" sx={{ fontSize: "0.7rem", color: "text.secondary" }}>
            {t("call.answer")}
          </Typography>
        </Box>
      </Box>
    );
  }

  const connectedCalls = activeCalls.filter(
    (c) => c.state === "connected" || c.state === "held" || c.state === "conferenced"
  );
  const heldCalls = activeCalls.filter((c) => c.state === "held");
  const hasMultipleCalls = connectedCalls.length > 1;
  const isInConference = activeCall.conferenceId !== undefined;
  const canAddCall = activeCall.state === "connected" && !isInConference;
  const canMerge = hasMultipleCalls && !isInConference;

  const handleHangup = async () => {
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
      sipCallId: activeCall.sipCallId,
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
        setActiveCall({ ...activeCall, recording: false });
      } else {
        const path = await sipStartRecording(activeCall.id);
        setActiveCall({ ...activeCall, recording: true, recordingPath: path });
      }
    } catch (e) {
      log.error("Recording toggle failed:", e);
    }
  };

  const handleAddCall = async () => {
    if (!newCallUri.trim()) return;

    try {
      await sipHold(activeCall.id, true);
      updateCall(activeCall.id, { held: true, state: "held" });
      const newCallId = await sipAddCall(newCallUri.trim());
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
      createConference(callIds);
      log.info("Conference created:", conferenceId);
    } catch (e) {
      log.error("Merge failed:", e);
    }
  };

  const handleSwap = async () => {
    const otherCall = heldCalls.find((c) => c.id !== activeCall.id);
    if (!otherCall) return;

    try {
      await sipSwapCalls(activeCall.id, otherCall.id);
      updateCall(activeCall.id, { held: true, state: "held" });
      updateCall(otherCall.id, { held: false, state: "connected" });
      setPrimaryCall(otherCall.id);
    } catch (e) {
      log.error("Swap failed:", e);
    }
  };

  const handleSelectDevice = (type: "input" | "output", name: string) => {
    if (type === "input") {
      setSelectedInputDevice(name);
    } else {
      setSelectedOutputDevice(name);
    }
    // Push to backend
    invoke("set_audio_devices", {
      inputDevice: type === "input" ? name : selectedInputDevice,
      outputDevice: type === "output" ? name : selectedOutputDevice,
    }).catch(() => {});
  };

  const hasNonDefaultDevice = audioDevices && (
    audioDevices.inputDevices.length > 1 || audioDevices.outputDevices.length > 1
  );

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
            active={Boolean(dtmfAnchor)}
            onClick={(e) => setDtmfAnchor(dtmfAnchor ? null : e.currentTarget)}
          />
          <ControlButton
            icon={<HeadsetMicIcon />}
            label={t("call.audio")}
            onClick={(e) => setAudioAnchor(e.currentTarget)}
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
          {canAddCall && (
            <ControlButton
              icon={<AddIcCallIcon />}
              label={t("call.addCall", "Add Call")}
              onClick={() => setAddCallDialogOpen(true)}
            />
          )}
        </Box>

        {/* Conference controls row */}
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

      {/* DTMF keypad popover */}
      <Popover
        open={Boolean(dtmfAnchor)}
        anchorEl={dtmfAnchor}
        onClose={() => setDtmfAnchor(null)}
        anchorOrigin={{ vertical: "top", horizontal: "center" }}
        transformOrigin={{ vertical: "bottom", horizontal: "center" }}
        slotProps={{
          paper: {
            sx: {
              borderRadius: "16px",
              p: 2,
              mb: 1,
            },
          },
        }}
      >
        <Box
          sx={{
            display: "grid",
            gridTemplateColumns: "repeat(3, 1fr)",
            gap: 1,
            justifyItems: "center",
          }}
        >
          {[
            { digit: "1", letters: "" },
            { digit: "2", letters: "ABC" },
            { digit: "3", letters: "DEF" },
            { digit: "4", letters: "GHI" },
            { digit: "5", letters: "JKL" },
            { digit: "6", letters: "MNO" },
            { digit: "7", letters: "PQRS" },
            { digit: "8", letters: "TUV" },
            { digit: "9", letters: "WXYZ" },
            { digit: "*", letters: "" },
            { digit: "0", letters: "+" },
            { digit: "#", letters: "" },
          ].map(({ digit, letters }) => (
            <DialerButton
              key={digit}
              digit={digit}
              letters={letters || undefined}
              onPress={(d) => {
                sipSendDtmf(activeCall.id, d).catch(() => {});
              }}
            />
          ))}
        </Box>
      </Popover>

      {/* Audio device picker popover */}
      <Popover
        open={Boolean(audioAnchor)}
        anchorEl={audioAnchor}
        onClose={() => setAudioAnchor(null)}
        anchorOrigin={{ vertical: "top", horizontal: "center" }}
        transformOrigin={{ vertical: "bottom", horizontal: "center" }}
        slotProps={{
          paper: {
            sx: {
              borderRadius: "16px",
              minWidth: 240,
              maxWidth: 300,
              mb: 1,
            },
          },
        }}
      >
        {audioDevices && (
          <Box sx={{ py: 1 }}>
            {/* Microphone section */}
            <Typography
              variant="overline"
              sx={{
                px: 2,
                py: 0.5,
                display: "block",
                fontSize: "0.6rem",
                color: "text.secondary",
                letterSpacing: 1,
              }}
            >
              {t("settings.microphone")}
            </Typography>
            <List dense disablePadding>
              {audioDevices.inputDevices.map((device) => {
                const isSelected = selectedInputDevice === device.name
                  || (!selectedInputDevice && device.isDefault);
                return (
                  <ListItemButton
                    key={device.name}
                    selected={isSelected}
                    onClick={() => handleSelectDevice("input", device.name)}
                    sx={{
                      py: 0.5,
                      px: 2,
                      borderRadius: "8px",
                      mx: 0.5,
                      minHeight: 36,
                    }}
                  >
                    <ListItemIcon sx={{ minWidth: 28 }}>
                      <MicIcon sx={{ fontSize: 16, color: isSelected ? "primary.main" : "text.secondary" }} />
                    </ListItemIcon>
                    <ListItemText
                      primary={device.name}
                      primaryTypographyProps={{
                        fontSize: "0.78rem",
                        fontWeight: isSelected ? 600 : 400,
                        noWrap: true,
                      }}
                    />
                    {isSelected && <CheckIcon sx={{ fontSize: 16, color: "primary.main" }} />}
                  </ListItemButton>
                );
              })}
            </List>

            {hasNonDefaultDevice && <Divider sx={{ my: 0.5 }} />}

            {/* Speaker section */}
            <Typography
              variant="overline"
              sx={{
                px: 2,
                py: 0.5,
                display: "block",
                fontSize: "0.6rem",
                color: "text.secondary",
                letterSpacing: 1,
              }}
            >
              {t("settings.speaker")}
            </Typography>
            <List dense disablePadding>
              {audioDevices.outputDevices.map((device) => {
                const isSelected = selectedOutputDevice === device.name
                  || (!selectedOutputDevice && device.isDefault);
                return (
                  <ListItemButton
                    key={device.name}
                    selected={isSelected}
                    onClick={() => handleSelectDevice("output", device.name)}
                    sx={{
                      py: 0.5,
                      px: 2,
                      borderRadius: "8px",
                      mx: 0.5,
                      minHeight: 36,
                    }}
                  >
                    <ListItemIcon sx={{ minWidth: 28 }}>
                      <VolumeUpIcon sx={{ fontSize: 16, color: isSelected ? "primary.main" : "text.secondary" }} />
                    </ListItemIcon>
                    <ListItemText
                      primary={device.name}
                      primaryTypographyProps={{
                        fontSize: "0.78rem",
                        fontWeight: isSelected ? 600 : 400,
                        noWrap: true,
                      }}
                    />
                    {isSelected && <CheckIcon sx={{ fontSize: 16, color: "primary.main" }} />}
                  </ListItemButton>
                );
              })}
            </List>

            <Typography
              variant="caption"
              sx={{
                display: "block",
                px: 2,
                pt: 1,
                pb: 0.5,
                fontSize: "0.6rem",
                color: "text.disabled",
              }}
            >
              {t("settings.audioNote")}
            </Typography>
          </Box>
        )}
      </Popover>

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
